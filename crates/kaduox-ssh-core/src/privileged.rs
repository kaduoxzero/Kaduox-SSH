use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::client::{CommandOutput, RemoteUser, SshClient, quote_posix};
use crate::transfer::{TransferOptions, unique_staging_path};

fn validate_mode(mode: u32) -> Result<()> {
    if mode > 0o7777 {
        bail!("invalid file mode {mode:#o}; expected <= 0o7777");
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
        // Privileged /tmp staging must be fail-closed too. The transfer policy
        // creates an exclusive temporary object and only renames it into this
        // unique staging name when the destination is still absent.
        staging_options.atomic = true;

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
}
