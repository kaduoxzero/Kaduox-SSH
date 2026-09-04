use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub(crate) fn verify_user_config_file(path: &Path, home: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect OpenSSH config {}", path.display()))?;
    if !metadata.is_file() {
        bail!("OpenSSH config path is not a regular file: {}", path.display());
    }

    verify_platform_trust(path, home, &metadata)
}

#[cfg(unix)]
fn verify_platform_trust(path: &Path, home: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let home_metadata = fs::metadata(home)
        .with_context(|| format!("failed to inspect user home directory {}", home.display()))?;
    if !home_metadata.is_dir() {
        bail!("configured user home is not a directory: {}", home.display());
    }

    let owner = metadata.uid();
    let home_owner = home_metadata.uid();
    if owner != home_owner && owner != 0 {
        bail!(
            "OpenSSH config {} is owned by uid {owner}; expected the home owner uid {home_owner} or root",
            path.display()
        );
    }

    let mode = metadata.mode() & 0o7777;
    if mode & 0o022 != 0 {
        bail!(
            "OpenSSH config {} has insecure permissions {mode:#06o}; group/other write bits must be clear",
            path.display()
        );
    }

    Ok(())
}

#[cfg(not(unix))]
fn verify_platform_trust(_path: &Path, _home: &Path, _metadata: &fs::Metadata) -> Result<()> {
    // Windows ACL semantics are not equivalent to POSIX ownership/mode bits.
    // Keep the cross-platform boundary explicit instead of pretending a Unix
    // policy can validate an NTFS ACL. Regular-file validation still applies.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kaduox-openssh-trust-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn regular_user_config_is_accepted() {
        let home = temp_root("regular");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        fs::write(&config, "Host prod\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        }

        verify_user_config_file(&config, &home).unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn directory_is_rejected_as_config_file() {
        let home = temp_root("directory");
        let ssh = home.join(".ssh");
        let config = ssh.join("config");
        fs::create_dir_all(&config).unwrap();

        assert!(verify_user_config_file(&config, &home).is_err());
        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn group_or_other_writable_config_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let home = temp_root("writable");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        fs::write(&config, "Host prod\n").unwrap();

        for mode in [0o620, 0o602, 0o666] {
            fs::set_permissions(&config, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                verify_user_config_file(&config, &home).is_err(),
                "mode {mode:#o}"
            );
        }

        for mode in [0o600, 0o640, 0o644] {
            fs::set_permissions(&config, fs::Permissions::from_mode(mode)).unwrap();
            verify_user_config_file(&config, &home).unwrap();
        }

        fs::remove_dir_all(home).unwrap();
    }
}