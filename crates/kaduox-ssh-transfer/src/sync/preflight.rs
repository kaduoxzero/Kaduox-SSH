use std::collections::BTreeMap;

use anyhow::{Result, bail};
use russh_sftp::client::{SftpSession, error::Error as SftpError};
use russh_sftp::protocol::StatusCode;

use crate::remote_path::{join_remote_under_root, validate_remote_relative_path};

use super::{SyncActionKind, SyncPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteNodeKind {
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PlannedMutation {
    delete_file: Option<usize>,
    delete_directory: Option<usize>,
    create_directory: Option<usize>,
}

impl PlannedMutation {
    fn has_delete(self) -> bool {
        self.delete_file.is_some() || self.delete_directory.is_some()
    }

    fn deletes_file(self) -> bool {
        self.delete_file.is_some()
    }

    fn deletes_directory(self) -> bool {
        self.delete_directory.is_some()
    }

    fn creates_directory(self) -> bool {
        self.create_directory.is_some()
    }
}

pub(super) async fn preflight_atomic_sync(
    sftp: &SftpSession,
    remote_root: &str,
    plan: &SyncPlan,
) -> Result<()> {
    let mutations = collect_planned_mutations(plan)?;
    let root_exists = remote_root_exists_without_symlinks(sftp, remote_root).await?;

    if !root_exists {
        if let Some(action) = plan.actions.iter().find(|action| {
            matches!(
                action.kind,
                SyncActionKind::DeleteRemoteFile | SyncActionKind::DeleteRemoteDirectory
            )
        }) {
            bail!(
                "atomic sync plan is stale: remote root {remote_root:?} is absent but the plan still deletes {:?} at {}; refusing mutation before upload preflight completes",
                action.kind,
                action.path
            );
        }

        for action in plan
            .actions
            .iter()
            .filter(|action| action.kind == SyncActionKind::UploadFile)
        {
            validate_upload_beneath_fresh_ancestor(
                &action.path,
                mutation_for(&mutations, &action.path),
            )?;
        }
        return Ok(());
    }

    for action in plan
        .actions
        .iter()
        .filter(|action| action.kind == SyncActionKind::UploadFile)
    {
        preflight_atomic_upload_path(sftp, remote_root, &action.path, &mutations).await?;
    }
    Ok(())
}

fn collect_planned_mutations<'a>(plan: &'a SyncPlan) -> Result<BTreeMap<&'a str, PlannedMutation>> {
    let mut mutations: BTreeMap<&'a str, PlannedMutation> = BTreeMap::new();
    for (index, action) in plan.actions.iter().enumerate() {
        let mutation = mutations.entry(action.path.as_str()).or_default();
        let slot = match action.kind {
            SyncActionKind::UploadFile => continue,
            SyncActionKind::DeleteRemoteFile => &mut mutation.delete_file,
            SyncActionKind::DeleteRemoteDirectory => &mut mutation.delete_directory,
            SyncActionKind::CreateRemoteDirectory => &mut mutation.create_directory,
        };
        if slot.replace(index).is_some() {
            bail!(
                "sync plan contains duplicate {:?} mutation for {}",
                action.kind,
                action.path
            );
        }
    }

    for (path, mutation) in &mutations {
        if mutation.deletes_file() && mutation.deletes_directory() {
            bail!("sync plan contains contradictory file/directory delete mutations for {path}");
        }
        if let Some(create_index) = mutation.create_directory {
            if let Some(delete_index) = mutation.delete_file.or(mutation.delete_directory) {
                if delete_index >= create_index {
                    bail!(
                        "sync plan must delete {path} before recreating it as a directory; delete action #{delete_index}, create action #{create_index}"
                    );
                }
            }
        }
    }
    Ok(mutations)
}

async fn preflight_atomic_upload_path(
    sftp: &SftpSession,
    remote_root: &str,
    relative: &str,
    mutations: &BTreeMap<&str, PlannedMutation>,
) -> Result<()> {
    validate_remote_relative_path(relative)?;
    let components = relative.split('/').collect::<Vec<_>>();
    let mut prefix = String::new();
    let mut ancestor_will_be_fresh = false;

    for (index, component) in components.iter().enumerate() {
        if prefix.is_empty() {
            prefix.push_str(component);
        } else {
            prefix.push('/');
            prefix.push_str(component);
        }

        let mutation = mutation_for(mutations, &prefix);
        let final_component = index + 1 == components.len();

        if ancestor_will_be_fresh {
            if final_component {
                return validate_upload_beneath_fresh_ancestor(&prefix, mutation);
            }
            validate_parent_beneath_fresh_ancestor(&prefix, mutation)?;
            continue;
        }

        let remote_path = join_remote_under_root(remote_root, &prefix)?;
        let current = remote_node_kind_if_exists(sftp, &remote_path).await?;

        if final_component {
            return validate_final_upload_destination(&remote_path, current, mutation);
        }

        match current {
            Some(RemoteNodeKind::Directory) => {
                if mutation.has_delete() {
                    bail!(
                        "atomic sync plan would delete existing parent directory {remote_path} before uploading beneath it; refusing ambiguous parent replacement"
                    );
                }
                // A redundant CreateRemoteDirectory is harmless for an existing
                // validated directory because ensure_remote_dir is idempotent.
            }
            Some(RemoteNodeKind::Symlink) | Some(RemoteNodeKind::Other) => {
                if mutation.deletes_file()
                    && !mutation.deletes_directory()
                    && mutation.creates_directory()
                {
                    // Generated type-conflict plans delete the non-directory
                    // entry and create a fresh directory before the upload phase.
                    // Do not inspect descendants through the current object.
                    ancestor_will_be_fresh = true;
                } else {
                    bail!(
                        "refusing atomic sync preflight through non-directory or symbolic-link parent {remote_path}; the plan must replace that parent with an explicit delete-file + create-directory pair before uploads"
                    );
                }
            }
            None => {
                if mutation.has_delete() {
                    bail!(
                        "atomic sync plan is stale: parent {remote_path} is absent but the plan contains a delete mutation"
                    );
                }
                // Missing parents will be created either by explicit directory
                // actions or by the checked upload parent creation path. No
                // descendant can currently occupy the eventual upload target.
                ancestor_will_be_fresh = true;
            }
        }
    }
    Ok(())
}

fn validate_parent_beneath_fresh_ancestor(path: &str, mutation: PlannedMutation) -> Result<()> {
    if mutation.has_delete() {
        bail!(
            "atomic sync plan is stale: {path} will be beneath a freshly created parent but also has a delete mutation"
        );
    }
    Ok(())
}

fn validate_upload_beneath_fresh_ancestor(path: &str, mutation: PlannedMutation) -> Result<()> {
    if mutation.has_delete() {
        bail!(
            "atomic sync plan is stale: upload destination {path} will be absent after parent creation but also has a delete mutation"
        );
    }
    if mutation.creates_directory() {
        bail!(
            "sync plan cannot create a directory and upload a regular file at the same path: {path}"
        );
    }
    Ok(())
}

fn validate_final_upload_destination(
    remote_path: &str,
    current: Option<RemoteNodeKind>,
    mutation: PlannedMutation,
) -> Result<()> {
    if mutation.creates_directory() {
        bail!(
            "sync plan cannot create a directory and upload a regular file at the same path: {remote_path}"
        );
    }

    match current {
        None => {
            if mutation.has_delete() {
                bail!(
                    "atomic sync plan is stale: upload destination {remote_path} is absent but the plan also deletes it"
                );
            }
            Ok(())
        }
        Some(RemoteNodeKind::Directory) => {
            if mutation.deletes_directory() && !mutation.deletes_file() {
                Ok(())
            } else if mutation.deletes_file() {
                bail!(
                    "atomic sync plan expects to delete a file at {remote_path}, but the current destination is a directory"
                )
            } else {
                reject_atomic_overwrite(remote_path)
            }
        }
        Some(RemoteNodeKind::Symlink) | Some(RemoteNodeKind::Other) => {
            if mutation.deletes_file() && !mutation.deletes_directory() {
                Ok(())
            } else if mutation.deletes_directory() {
                bail!(
                    "atomic sync plan expects to delete a directory at {remote_path}, but the current destination is not a directory"
                )
            } else {
                reject_atomic_overwrite(remote_path)
            }
        }
    }
}

fn reject_atomic_overwrite(path: &str) -> Result<()> {
    bail!(
        "atomic sync upload would replace existing remote destination {path}, but overwrite-atomic rename is unavailable with the current SFTP v3 API; rerun with --no-atomic only if a non-atomic replacement is acceptable"
    )
}

fn mutation_for(mutations: &BTreeMap<&str, PlannedMutation>, path: &str) -> PlannedMutation {
    mutations.get(path).copied().unwrap_or_default()
}

async fn remote_root_exists_without_symlinks(sftp: &SftpSession, root: &str) -> Result<bool> {
    if root.is_empty() || root == "." || root == "/" {
        return Ok(true);
    }
    if root.contains('\0') {
        bail!("remote sync root cannot contain NUL bytes");
    }

    let absolute = root.starts_with('/');
    let mut current = if absolute {
        "/".to_owned()
    } else {
        String::new()
    };
    for segment in root.split('/').filter(|segment| !segment.is_empty()) {
        if segment == "." {
            continue;
        }
        if segment == ".." {
            bail!("remote sync root traversal with '..' is not supported: {root}");
        }
        current = join_remote_component(&current, segment);
        match remote_node_kind_if_exists(sftp, &current).await? {
            Some(RemoteNodeKind::Directory) => {}
            Some(RemoteNodeKind::Symlink) => {
                bail!(
                    "refusing to follow symbolic-link component in remote sync destination root: {current}"
                )
            }
            Some(RemoteNodeKind::Other) => {
                bail!("remote sync destination root component is not a directory: {current}")
            }
            None => return Ok(false),
        }
    }
    Ok(true)
}

async fn remote_node_kind_if_exists(
    sftp: &SftpSession,
    path: &str,
) -> Result<Option<RemoteNodeKind>> {
    match sftp.symlink_metadata(path.to_owned()).await {
        Ok(metadata) if metadata.is_symlink() => Ok(Some(RemoteNodeKind::Symlink)),
        Ok(metadata) if metadata.is_dir() => Ok(Some(RemoteNodeKind::Directory)),
        Ok(_) => Ok(Some(RemoteNodeKind::Other)),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn join_remote_component(parent: &str, child: &str) -> String {
    if parent.is_empty() || parent == "." {
        child.to_owned()
    } else if parent == "/" {
        format!("/{child}")
    } else if parent.ends_with('/') {
        format!("{parent}{child}")
    } else {
        format!("{parent}/{child}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncAction;

    fn mutation(
        delete_file: Option<usize>,
        delete_directory: Option<usize>,
        create_directory: Option<usize>,
    ) -> PlannedMutation {
        PlannedMutation {
            delete_file,
            delete_directory,
            create_directory,
        }
    }

    #[test]
    fn existing_file_requires_matching_delete_for_atomic_upload() {
        assert!(
            validate_final_upload_destination(
                "/srv/app.bin",
                Some(RemoteNodeKind::Other),
                PlannedMutation::default(),
            )
            .unwrap_err()
            .to_string()
            .contains("--no-atomic")
        );
        validate_final_upload_destination(
            "/srv/app.bin",
            Some(RemoteNodeKind::Other),
            mutation(Some(0), None, None),
        )
        .unwrap();
    }

    #[test]
    fn existing_directory_requires_directory_delete_for_file_replacement() {
        validate_final_upload_destination(
            "/srv/app",
            Some(RemoteNodeKind::Directory),
            mutation(None, Some(0), None),
        )
        .unwrap();
        assert!(
            validate_final_upload_destination(
                "/srv/app",
                Some(RemoteNodeKind::Directory),
                mutation(Some(0), None, None),
            )
            .is_err()
        );
    }

    #[test]
    fn absent_atomic_upload_destination_rejects_stale_delete() {
        validate_final_upload_destination("/srv/new.bin", None, PlannedMutation::default())
            .unwrap();
        assert!(
            validate_final_upload_destination("/srv/new.bin", None, mutation(Some(0), None, None),)
                .is_err()
        );
    }

    #[test]
    fn upload_and_create_directory_same_path_is_rejected() {
        assert!(
            validate_upload_beneath_fresh_ancestor("release", mutation(None, None, Some(0)),)
                .is_err()
        );
    }

    #[test]
    fn duplicate_or_contradictory_mutations_are_rejected() {
        let duplicate = SyncPlan {
            actions: vec![
                SyncAction {
                    kind: SyncActionKind::DeleteRemoteFile,
                    path: "app.bin".to_owned(),
                    bytes: 0,
                },
                SyncAction {
                    kind: SyncActionKind::DeleteRemoteFile,
                    path: "app.bin".to_owned(),
                    bytes: 0,
                },
            ],
            ..Default::default()
        };
        assert!(collect_planned_mutations(&duplicate).is_err());

        let contradictory = SyncPlan {
            actions: vec![
                SyncAction {
                    kind: SyncActionKind::DeleteRemoteFile,
                    path: "app".to_owned(),
                    bytes: 0,
                },
                SyncAction {
                    kind: SyncActionKind::DeleteRemoteDirectory,
                    path: "app".to_owned(),
                    bytes: 0,
                },
            ],
            ..Default::default()
        };
        assert!(collect_planned_mutations(&contradictory).is_err());
    }

    #[test]
    fn delete_must_precede_same_path_directory_recreation() {
        let safe = SyncPlan {
            actions: vec![
                SyncAction {
                    kind: SyncActionKind::DeleteRemoteFile,
                    path: "app".to_owned(),
                    bytes: 0,
                },
                SyncAction {
                    kind: SyncActionKind::CreateRemoteDirectory,
                    path: "app".to_owned(),
                    bytes: 0,
                },
            ],
            ..Default::default()
        };
        collect_planned_mutations(&safe).unwrap();

        let unsafe_order = SyncPlan {
            actions: vec![
                SyncAction {
                    kind: SyncActionKind::CreateRemoteDirectory,
                    path: "app".to_owned(),
                    bytes: 0,
                },
                SyncAction {
                    kind: SyncActionKind::DeleteRemoteFile,
                    path: "app".to_owned(),
                    bytes: 0,
                },
            ],
            ..Default::default()
        };
        assert!(collect_planned_mutations(&unsafe_order).is_err());
    }

    #[test]
    fn fresh_ancestor_rejects_stale_delete_but_allows_directory_creation() {
        validate_parent_beneath_fresh_ancestor("app/assets", mutation(None, None, Some(0)))
            .unwrap();
        assert!(
            validate_parent_beneath_fresh_ancestor("app/assets", mutation(Some(0), None, Some(1)),)
                .is_err()
        );
    }
}
