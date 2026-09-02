use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::client::{CommandOutput, RemoteUser, SshClient, quote_posix};
use crate::transfer::{TransferOptions, TransferSummary, unique_staging_path};

fn validate_mode(mode: u32) -> Result<()> {
    if mode > 0o7777 {
        bail!("invalid file mode {mode:#o}; expected <= 0o7777");
    }
    Ok(())
}

fn validate_recursive_destination(path: &str) -> Result<()> {
    if path.is_empty() || path == "/" {
        bail!("privileged recursive destination must not be empty or the filesystem root");
    }
    if path
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        bail!(
            "privileged recursive destination must not contain '.' or '..' path components: {path}"
        );
    }
    Ok(())
}

fn ensure_privileged_install_success(output: &CommandOutput) -> Result<()> {
    match output.exit_status {
        Some(0) => Ok(()),
        Some(status) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "privileged install failed with status {status}: {}",
                stderr.trim()
            )
        }
        None => bail!("privileged install failed: remote command did not report an exit status"),
    }
}

fn privileged_tree_install_command(
    staging_path: &str,
    remote_path: &str,
    work_path: &str,
    file_mode: u32,
    directory_mode: u32,
) -> String {
    let staging = quote_posix(staging_path);
    let target = quote_posix(remote_path);
    let work = quote_posix(work_path);
    format!(
        "set -e; rm -rf -- {work}; install -d -m {directory_mode:o} -- {work}; cp -R -- {staging}/. {work}/; find {work} -type d -exec chmod {directory_mode:o} -- {{}} +; find {work} -type f -exec chmod {file_mode:o} -- {{}} +; if [ -e {target} ] || [ -L {target} ]; then rm -rf -- {target}; fi; mv -- {work} {target}"
    )
}

impl SshClient {
    /// Upload a file as the SSH login user, then atomically install it as another
    /// remote OS user through sudo. The SFTP server itself never changes uid.
    pub async fn upload_privileged(
        &self,
        local_path: &Path,
        remote_path: &str,
        as_user: &str,
        mode: u32,
        options: TransferOptions,
    ) -> Result<u64> {
        validate_mode(mode)?;
        if remote_path.is_empty() {
            bail!("remote path cannot be empty");
        }

        let file_name = local_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("upload");
        let staging_path = unique_staging_path(file_name);
        let privileged_work_path = format!("{remote_path}.kaduox.privileged.part");

        let mut staging_options = options;
        staging_options.resume = false;
        staging_options.atomic = false;

        let bytes = self
            .upload_with_options(local_path, &staging_path, staging_options)
            .await
            .with_context(|| format!("failed to stage upload at {staging_path}"))?;

        let install_command = format!(
            "install -m {mode:o} -- {} {} && mv -f -- {} {}",
            quote_posix(&staging_path),
            quote_posix(&privileged_work_path),
            quote_posix(&privileged_work_path),
            quote_posix(remote_path),
        );
        let result = self
            .exec(&install_command, &RemoteUser::Sudo(as_user.to_owned()))
            .await;

        let cleanup_command = format!("rm -f -- {}", quote_posix(&staging_path));
        let _ = self.exec(&cleanup_command, &RemoteUser::Current).await;

        let output = result?;
        ensure_privileged_install_success(&output)?;
        Ok(bytes)
    }

    /// Recursively upload a directory as the SSH login user, then install the
    /// staged tree as another remote OS user through sudo.
    ///
    /// Recursive SFTP staging keeps the normal bounded file concurrency. The
    /// privileged phase never runs SFTP as the target user and deliberately
    /// does not follow symbolic links because the regular recursive uploader
    /// skips them. Replacing an existing destination directory requires a
    /// remove-then-rename step and therefore is not claimed to be atomic.
    pub async fn upload_privileged_recursive(
        &self,
        local_path: &Path,
        remote_path: &str,
        as_user: &str,
        file_mode: u32,
        directory_mode: u32,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        validate_mode(file_mode)?;
        validate_mode(directory_mode)?;
        validate_recursive_destination(remote_path)?;

        let metadata = tokio::fs::symlink_metadata(local_path)
            .await
            .with_context(|| format!("failed to stat {}", local_path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!(
                "privileged recursive upload source {} must be a directory and cannot be a symbolic link",
                local_path.display()
            );
        }

        let file_name = local_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tree");
        let staging_path = unique_staging_path(file_name);
        let privileged_work_path = format!("{remote_path}.kaduox.privileged.part");

        let mut staging_options = options;
        staging_options.resume = false;
        staging_options.atomic = false;

        let summary = self
            .upload_recursive(local_path, &staging_path, staging_options)
            .await
            .with_context(|| format!("failed to stage recursive upload at {staging_path}"))?;

        let install_command = privileged_tree_install_command(
            &staging_path,
            remote_path,
            &privileged_work_path,
            file_mode,
            directory_mode,
        );
        let result = self
            .exec(&install_command, &RemoteUser::Sudo(as_user.to_owned()))
            .await;

        let staging_cleanup = format!("rm -rf -- {}", quote_posix(&staging_path));
        let _ = self.exec(&staging_cleanup, &RemoteUser::Current).await;
        let work_cleanup = format!("rm -rf -- {}", quote_posix(&privileged_work_path));
        let _ = self
            .exec(&work_cleanup, &RemoteUser::Sudo(as_user.to_owned()))
            .await;

        let output = result?;
        ensure_privileged_install_success(&output)?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_range_accepts_unix_special_bits() {
        assert!(validate_mode(0o7777).is_ok());
        assert!(validate_mode(0o10000).is_err());
    }

    #[test]
    fn recursive_destination_rejects_destructive_paths() {
        assert!(validate_recursive_destination("/").is_err());
        assert!(validate_recursive_destination(".").is_err());
        assert!(validate_recursive_destination("..").is_err());
        assert!(validate_recursive_destination("/srv/../etc").is_err());
        assert!(validate_recursive_destination("/srv/app").is_ok());
    }

    #[test]
    fn privileged_install_requires_explicit_success_status() {
        let missing = CommandOutput::default();
        assert!(ensure_privileged_install_success(&missing).is_err());

        let success = CommandOutput {
            exit_status: Some(0),
            ..Default::default()
        };
        assert!(ensure_privileged_install_success(&success).is_ok());

        let failure = CommandOutput {
            stderr: b"permission denied".to_vec(),
            exit_status: Some(1),
            ..Default::default()
        };
        let error = ensure_privileged_install_success(&failure).unwrap_err();
        assert!(error.to_string().contains("permission denied"));
    }

    #[test]
    fn recursive_install_command_quotes_paths_and_modes() {
        let command = privileged_tree_install_command(
            "/tmp/stage dir",
            "/srv/app dir",
            "/srv/app dir.kaduox.privileged.part",
            0o640,
            0o750,
        );
        assert!(command.contains("chmod 640"));
        assert!(command.contains("chmod 750"));
        assert!(command.contains("'/tmp/stage dir'/"));
        assert!(command.contains("'/srv/app dir'"));
    }
}
