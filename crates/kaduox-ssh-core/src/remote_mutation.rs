use anyhow::{Context, Result, bail};
use russh_sftp::client::{SftpSession, error::Error as SftpError};
use russh_sftp::protocol::StatusCode;

use crate::client::SshClient;
use crate::remote_fs::{RemoteFileType, list_directory, stat_path};
use crate::transfer::TransferOptions;

impl SshClient {
    /// Create one remote directory without creating missing parents.
    ///
    /// The target must be an unambiguous non-root path and must not already
    /// exist, including as a symbolic link. This operation deliberately does
    /// not provide `mkdir -p` semantics.
    pub async fn create_remote_directory(&self, path: &str) -> Result<()> {
        validate_mutation_path(path)?;
        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = create_directory(&sftp, path).await;
        let close_result = sftp.close().await;
        result?;
        close_result.context("failed to close SFTP session after remote mkdir")?;
        Ok(())
    }

    /// Rename one remote path within its current directory.
    ///
    /// Cross-directory moves and occupied destinations are rejected. SFTP v3
    /// rename semantics remain authoritative at mutation time, so a target
    /// created after preflight must still cause the server rename to fail.
    pub async fn rename_remote_path(
        &self,
        source: &str,
        destination: &str,
    ) -> Result<RemoteFileType> {
        validate_mutation_path(source)?;
        validate_mutation_path(destination)?;
        ensure_same_parent(source, destination)?;
        if source == destination {
            bail!("remote rename source and destination are identical");
        }

        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = rename_path(&sftp, source, destination).await;
        let close_result = sftp.close().await;
        let file_type = result?;
        close_result.context("failed to close SFTP session after remote rename")?;
        Ok(file_type)
    }

    /// Remove one regular file, symbolic link, or empty directory.
    ///
    /// Recursive deletion is intentionally not implemented here. Directories
    /// are listed immediately before `rmdir`, while the protocol/server remains
    /// responsible for rejecting a directory that becomes non-empty in a race.
    pub async fn remove_remote_path(&self, path: &str) -> Result<RemoteFileType> {
        validate_mutation_path(path)?;
        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = remove_path(&sftp, path).await;
        let close_result = sftp.close().await;
        let file_type = result?;
        close_result.context("failed to close SFTP session after remote removal")?;
        Ok(file_type)
    }
}

async fn create_directory(sftp: &SftpSession, path: &str) -> Result<()> {
    if path_exists_no_follow(sftp, path).await? {
        bail!("remote directory target already exists: {path}");
    }
    sftp.create_dir(path.to_owned())
        .await
        .with_context(|| format!("failed to create remote directory {path}"))
}

async fn rename_path(
    sftp: &SftpSession,
    source: &str,
    destination: &str,
) -> Result<RemoteFileType> {
    let source_stat = stat_path(sftp, source)
        .await
        .with_context(|| format!("failed to preflight remote rename source {source}"))?;
    if path_exists_no_follow(sftp, destination).await? {
        bail!("remote rename destination already exists: {destination}");
    }

    sftp.rename(source.to_owned(), destination.to_owned())
        .await
        .with_context(|| format!("failed to rename remote path {source} to {destination}"))?;
    Ok(source_stat.metadata.file_type)
}

async fn remove_path(sftp: &SftpSession, path: &str) -> Result<RemoteFileType> {
    let stat = stat_path(sftp, path)
        .await
        .with_context(|| format!("failed to preflight remote removal {path}"))?;
    let file_type = stat.metadata.file_type;

    match file_type {
        RemoteFileType::File | RemoteFileType::Symlink => {
            sftp.remove_file(path.to_owned())
                .await
                .with_context(|| format!("failed to remove remote path {path}"))?;
        }
        RemoteFileType::Directory => {
            let entries = list_directory(sftp, path)
                .await
                .with_context(|| format!("failed to verify remote directory is empty: {path}"))?;
            if !entries.is_empty() {
                bail!(
                    "refusing recursive remote deletion: directory is not empty: {path}"
                );
            }
            sftp.remove_dir(path.to_owned())
                .await
                .with_context(|| format!("failed to remove empty remote directory {path}"))?;
        }
        RemoteFileType::Other => {
            bail!("refusing to remove unsupported remote file type: {path}");
        }
    }

    Ok(file_type)
}

async fn path_exists_no_follow(sftp: &SftpSession, path: &str) -> Result<bool> {
    match sftp.symlink_metadata(path.to_owned()).await {
        Ok(_) => Ok(true),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect remote path occupancy: {path}")),
    }
}

fn validate_mutation_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("remote mutation path must not be empty");
    }
    if path.chars().any(char::is_control) || path.contains('\0') {
        bail!("remote mutation path must not contain control characters or NUL");
    }
    if path == "/" || path == "." {
        bail!("refusing to mutate the remote root/current-directory path");
    }

    let body = path
        .strip_prefix("./")
        .or_else(|| path.strip_prefix('/'))
        .unwrap_or(path);
    if body.is_empty() || body.ends_with('/') {
        bail!("remote mutation path must name one concrete entry");
    }
    for component in body.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            bail!("remote mutation path contains an ambiguous path component");
        }
    }
    Ok(())
}

fn ensure_same_parent(source: &str, destination: &str) -> Result<()> {
    if remote_parent(source) != remote_parent(destination) {
        bail!("v0.10 remote rename is restricted to the same directory");
    }
    Ok(())
}

fn remote_parent(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => ".",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_paths_are_unambiguous_and_non_root() {
        for valid in [
            "file.txt",
            "./file.txt",
            "dir/file.txt",
            "/var/log/file.txt",
            "dir/name with spaces",
        ] {
            assert!(validate_mutation_path(valid).is_ok(), "{valid:?}");
        }

        for invalid in [
            "",
            ".",
            "/",
            "../escape",
            "./../escape",
            "dir/../escape",
            "dir/./file",
            "dir//file",
            "dir/",
            "bad\0name",
            "line\nname",
        ] {
            assert!(validate_mutation_path(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn rename_is_same_directory_only() {
        assert!(ensure_same_parent("/srv/a", "/srv/b").is_ok());
        assert!(ensure_same_parent("./a", "./b").is_ok());
        assert!(ensure_same_parent("a", "b").is_ok());
        assert!(ensure_same_parent("/srv/a", "/tmp/a").is_err());
        assert!(ensure_same_parent("dir/a", "dir2/a").is_err());
    }

    #[test]
    fn parent_resolution_handles_root_relative_and_dot_prefix() {
        assert_eq!(remote_parent("/file"), "/");
        assert_eq!(remote_parent("/var/file"), "/var");
        assert_eq!(remote_parent("file"), ".");
        assert_eq!(remote_parent("./file"), ".");
    }
}
