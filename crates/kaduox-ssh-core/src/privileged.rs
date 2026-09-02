use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::client::{RemoteUser, SshClient, quote_posix};
use crate::transfer::{TransferOptions, unique_staging_path};

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
        if mode > 0o7777 {
            bail!("invalid file mode {mode:#o}; expected <= 0o7777");
        }
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
        if output.exit_status.unwrap_or(0) != 0 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "privileged install failed with status {:?}: {}",
                output.exit_status,
                stderr.trim()
            );
        }

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mode_range_matches_unix_special_bits() {
        assert!(0o7777_u32 <= 0o7777);
        assert!(0o10000_u32 > 0o7777);
    }
}
