use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

const MAX_CONFIG_FILE_BYTES: usize = 4 * 1024 * 1024;

#[cfg(windows)]
mod windows_acl;

#[cfg(unix)]
unsafe extern "C" {
    fn getuid() -> std::os::raw::c_uint;
}

pub(crate) fn read_user_config_file(path: &Path, home: &Path) -> Result<String> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open OpenSSH config {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect OpenSSH config {}", path.display()))?;
    if !metadata.is_file() {
        bail!("OpenSSH config path is not a regular file: {}", path.display());
    }

    verify_platform_trust(path, home, &file, &metadata)?;

    let read_limit = u64::try_from(MAX_CONFIG_FILE_BYTES)
        .expect("4 MiB OpenSSH config limit fits u64")
        + 1;
    let mut limited = file.take(read_limit);
    let mut contents = String::new();
    limited
        .read_to_string(&mut contents)
        .with_context(|| format!("failed to read OpenSSH config {}", path.display()))?;
    if contents.len() > MAX_CONFIG_FILE_BYTES {
        bail!(
            "OpenSSH config {} exceeds the {MAX_CONFIG_FILE_BYTES}-byte per-file safety limit",
            path.display()
        );
    }
    Ok(contents)
}

#[cfg(unix)]
fn verify_platform_trust(
    path: &Path,
    _home: &Path,
    _file: &fs::File,
    metadata: &fs::Metadata,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let owner = metadata.uid();
    let uid = current_real_uid();
    if owner != uid && owner != 0 {
        bail!(
            "OpenSSH config {} is owned by uid {owner}; expected the current uid {uid} or root",
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

#[cfg(unix)]
fn current_real_uid() -> u32 {
    // SAFETY: getuid() is a side-effect-free POSIX process query with no
    // arguments or pointer requirements. Linux and macOS expose uid_t as an
    // unsigned integer compatible with c_uint, which are the Unix targets in
    // the supported CI/release matrix.
    unsafe { getuid() }
}

#[cfg(windows)]
fn verify_platform_trust(
    path: &Path,
    _home: &Path,
    file: &fs::File,
    _metadata: &fs::Metadata,
) -> Result<()> {
    windows_acl::verify_open_file_acl(path, file)
}

#[cfg(not(any(unix, windows)))]
fn verify_platform_trust(
    _path: &Path,
    _home: &Path,
    _file: &fs::File,
    _metadata: &fs::Metadata,
) -> Result<()> {
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
        let base = if cfg!(windows) {
            std::env::var_os("USERPROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
        } else {
            std::env::temp_dir()
        };
        base.join(format!(
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

        assert_eq!(read_user_config_file(&config, &home).unwrap(), "Host prod\n");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn directory_is_rejected_as_config_file() {
        let home = temp_root("directory");
        let ssh = home.join(".ssh");
        let config = ssh.join("config");
        fs::create_dir_all(&config).unwrap();

        assert!(read_user_config_file(&config, &home).is_err());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn oversized_config_file_is_rejected_before_unbounded_read() {
        let home = temp_root("oversized");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        fs::write(&config, vec![b'x'; MAX_CONFIG_FILE_BYTES + 1]).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert!(read_user_config_file(&config, &home).is_err());
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
                read_user_config_file(&config, &home).is_err(),
                "mode {mode:#o}"
            );
        }

        for mode in [0o600, 0o640, 0o644] {
            fs::set_permissions(&config, fs::Permissions::from_mode(mode)).unwrap();
            assert!(read_user_config_file(&config, &home).is_ok());
        }

        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn current_uid_matches_new_temp_file_owner() {
        use std::os::unix::fs::MetadataExt;

        let home = temp_root("uid");
        fs::create_dir_all(&home).unwrap();
        let file = home.join("owned");
        fs::write(&file, "x").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().uid(), current_real_uid());
        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(windows)]
    fn grant_builtin_users(config: &Path, rights: &str) {
        let grant = format!("*S-1-5-32-545:{rights}");
        let output = std::process::Command::new("icacls")
            .arg(config)
            .arg("/grant")
            .arg(grant)
            .output()
            .expect("run icacls");
        assert!(
            output.status.success(),
            "icacls failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_read_only_untrusted_ace_is_accepted() {
        let home = temp_root("windows-read");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        fs::write(&config, "Host prod\n").unwrap();

        assert!(read_user_config_file(&config, &home).is_ok());
        grant_builtin_users(&config, "(R)");
        assert_eq!(read_user_config_file(&config, &home).unwrap(), "Host prod\n");
        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_untrusted_write_ace_is_rejected() {
        let home = temp_root("windows-write");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        fs::write(&config, "Host prod\n").unwrap();

        assert!(read_user_config_file(&config, &home).is_ok());
        grant_builtin_users(&config, "(W)");
        let error = read_user_config_file(&config, &home).unwrap_err().to_string();
        assert!(error.contains("write-class Windows ACL rights"), "{error}");
        fs::remove_dir_all(home).unwrap();
    }
}
