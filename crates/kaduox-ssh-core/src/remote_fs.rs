use anyhow::{Context, Result, bail};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType as SftpFileType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFileType {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFileMetadata {
    pub file_type: RemoteFileType,
    pub size: Option<u64>,
    pub uid: Option<u32>,
    pub user: Option<String>,
    pub gid: Option<u32>,
    pub group: Option<String>,
    pub permissions: Option<u32>,
    pub accessed_at: Option<u32>,
    pub modified_at: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDirEntry {
    pub name: String,
    pub path: String,
    pub metadata: RemoteFileMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFileStat {
    pub path: String,
    pub metadata: RemoteFileMetadata,
    pub symlink_target: Option<String>,
}

pub(crate) async fn list_directory(sftp: &SftpSession, path: &str) -> Result<Vec<RemoteDirEntry>> {
    let directory = sftp
        .read_dir(path)
        .await
        .with_context(|| format!("failed to list remote directory {path}"))?;
    // `ReadDir` is only required to be iterable by the russh-sftp API. Avoid
    // relying on an ExactSizeIterator-style `len()` contract that is not part
    // of the public abstraction.
    let mut entries = Vec::new();

    for entry in directory {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        validate_remote_entry_name(&name)?;
        let file_type = map_file_type(entry.file_type());
        entries.push(RemoteDirEntry {
            path: join_remote_child(path, &name),
            name,
            metadata: map_metadata(entry.metadata(), file_type),
        });
    }

    entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

pub(crate) async fn stat_path(sftp: &SftpSession, path: &str) -> Result<RemoteFileStat> {
    let attrs = sftp
        .symlink_metadata(path)
        .await
        .with_context(|| format!("failed to stat remote path {path}"))?;
    let file_type = attrs
        .permissions
        .map(SftpFileType::from)
        .map(map_file_type)
        .unwrap_or(RemoteFileType::Other);
    let symlink_target = if file_type == RemoteFileType::Symlink {
        Some(
            sftp.read_link(path)
                .await
                .with_context(|| format!("failed to read remote symlink {path}"))?,
        )
    } else {
        None
    };

    Ok(RemoteFileStat {
        path: path.to_owned(),
        metadata: map_metadata(attrs, file_type),
        symlink_target,
    })
}

fn validate_remote_entry_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." {
        bail!("remote directory returned an invalid entry name");
    }
    if name.contains('/') || name.contains('\0') {
        bail!("remote directory returned a path-bearing entry name: {name:?}");
    }
    Ok(())
}

fn join_remote_child(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn map_metadata(attrs: FileAttributes, file_type: RemoteFileType) -> RemoteFileMetadata {
    RemoteFileMetadata {
        file_type,
        size: attrs.size,
        uid: attrs.uid,
        user: attrs.user,
        gid: attrs.gid,
        group: attrs.group,
        permissions: attrs.permissions,
        accessed_at: attrs.atime,
        modified_at: attrs.mtime,
    }
}

fn map_file_type(file_type: SftpFileType) -> RemoteFileType {
    match file_type {
        SftpFileType::Dir => RemoteFileType::Directory,
        SftpFileType::File => RemoteFileType::File,
        SftpFileType::Symlink => RemoteFileType::Symlink,
        SftpFileType::Other => RemoteFileType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_sftp_file_types_without_loss() {
        assert_eq!(map_file_type(SftpFileType::Dir), RemoteFileType::Directory);
        assert_eq!(map_file_type(SftpFileType::File), RemoteFileType::File);
        assert_eq!(
            map_file_type(SftpFileType::Symlink),
            RemoteFileType::Symlink
        );
        assert_eq!(map_file_type(SftpFileType::Other), RemoteFileType::Other);
    }

    #[test]
    fn maps_metadata_fields() {
        let attrs = FileAttributes {
            size: Some(123),
            uid: Some(1000),
            user: Some("deploy".to_owned()),
            gid: Some(1000),
            group: Some("deploy".to_owned()),
            permissions: Some(0o100640),
            atime: Some(11),
            mtime: Some(22),
        };
        let metadata = map_metadata(attrs, RemoteFileType::File);
        assert_eq!(metadata.size, Some(123));
        assert_eq!(metadata.permissions, Some(0o100640));
        assert_eq!(metadata.modified_at, Some(22));
        assert_eq!(metadata.user.as_deref(), Some("deploy"));
    }

    #[test]
    fn rejects_path_bearing_directory_entry_names() {
        for name in ["", ".", "..", "../escape", "nested/file", "bad\0name"] {
            assert!(validate_remote_entry_name(name).is_err(), "{name:?}");
        }
        assert!(validate_remote_entry_name("normal file.txt").is_ok());
    }

    #[test]
    fn reconstructs_remote_child_paths_from_parent_and_name() {
        assert_eq!(join_remote_child("/", "etc"), "/etc");
        assert_eq!(join_remote_child("/var/log", "syslog"), "/var/log/syslog");
        assert_eq!(join_remote_child("relative/", "file"), "relative/file");
    }
}
