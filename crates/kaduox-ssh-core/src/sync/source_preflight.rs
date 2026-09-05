use std::path::{Component, Path};

use anyhow::{Context, Result, bail};

use crate::remote_path::local_path_from_remote_relative;

use super::{SyncActionKind, SyncPlan};

/// Validate every local upload source in a public/stale SyncPlan before any
/// remote mutation begins.
///
/// SyncPlan is intentionally public. Callers can construct it directly and a
/// generated plan can also become stale before apply. Walking every component
/// with symlink_metadata prevents a symlinked parent from redirecting an upload
/// outside the selected local synchronization root during this preflight.
pub(super) async fn preflight_local_sync_sources(
    local_root: &Path,
    plan: &SyncPlan,
) -> Result<()> {
    for (index, action) in plan.actions.iter().enumerate() {
        if action.kind != SyncActionKind::UploadFile {
            continue;
        }

        let relative = local_path_from_remote_relative(&action.path).with_context(|| {
            format!("sync upload action #{index} has an unsafe local path")
        })?;
        preflight_local_sync_source(local_root, &relative, action.bytes)
            .await
            .with_context(|| {
                format!(
                    "sync upload action #{index} source preflight failed for {}",
                    action.path
                )
            })?;
    }
    Ok(())
}

async fn preflight_local_sync_source(
    local_root: &Path,
    relative: &Path,
    expected_len: u64,
) -> Result<()> {
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        bail!("sync upload source cannot be the synchronization root itself");
    }

    let mut current = local_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            bail!("sync upload source contains a non-normal local path component");
        };
        current.push(component);

        let metadata = tokio::fs::symlink_metadata(&current)
            .await
            .with_context(|| format!("failed to stat sync upload source {}", current.display()))?;
        if is_local_link_like(&metadata) {
            bail!(
                "refusing to follow symbolic-link/reparse component in sync upload source: {}",
                current.display()
            );
        }

        let final_component = index + 1 == components.len();
        if final_component {
            if !metadata.is_file() {
                bail!(
                    "sync upload source is no longer a regular file: {}",
                    current.display()
                );
            }
            if metadata.len() != expected_len {
                bail!(
                    "sync plan is stale: upload source size changed from {expected_len} to {} bytes: {}",
                    metadata.len(),
                    current.display()
                );
            }
        } else if !metadata.is_dir() {
            bail!(
                "sync upload source parent is no longer a directory: {}",
                current.display()
            );
        }
    }
    Ok(())
}

fn is_local_link_like(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::sync::SyncAction;

    static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let serial = TEMP_SERIAL.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "kaduox-sync-preflight-{label}-{}-{stamp:x}-{serial:x}",
            std::process::id()
        ))
    }

    fn upload_plan(path: &str, bytes: u64) -> SyncPlan {
        SyncPlan {
            actions: vec![SyncAction {
                kind: SyncActionKind::UploadFile,
                path: path.to_owned(),
                bytes,
            }],
            files_to_upload: 1,
            bytes_to_upload: bytes,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn accepts_regular_source_with_planned_size() {
        let root = temp_root("regular");
        tokio::fs::create_dir_all(root.join("nested")).await.unwrap();
        tokio::fs::write(root.join("nested/app.bin"), b"data")
            .await
            .unwrap();

        preflight_local_sync_sources(&root, &upload_plan("nested/app.bin", 4))
            .await
            .unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_stale_source_size_before_mutation() {
        let root = temp_root("stale-size");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("app.bin"), b"changed")
            .await
            .unwrap();

        let error = preflight_local_sync_sources(&root, &upload_plan("app.bin", 3))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("source preflight failed"));
        assert!(format!("{error:#}").contains("sync plan is stale"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_missing_source_before_mutation() {
        let root = temp_root("missing");
        tokio::fs::create_dir_all(&root).await.unwrap();

        let error = preflight_local_sync_sources(&root, &upload_plan("missing.bin", 1))
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("failed to stat sync upload source"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlinked_parent_that_escapes_source_root() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink-root");
        let outside = temp_root("symlink-outside");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        tokio::fs::write(outside.join("secret.bin"), b"secret")
            .await
            .unwrap();
        symlink(&outside, root.join("escape")).unwrap();

        let error = preflight_local_sync_sources(&root, &upload_plan("escape/secret.bin", 6))
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("symbolic-link/reparse component"));

        tokio::fs::remove_file(root.join("escape")).await.unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
        tokio::fs::remove_dir_all(outside).await.unwrap();
    }
}
