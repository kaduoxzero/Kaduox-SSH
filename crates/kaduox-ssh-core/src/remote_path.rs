use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

/// Validate one filename returned by an SFTP directory listing before it is
/// combined with a caller-controlled synchronization root.
pub(crate) fn validate_remote_child_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("remote directory entry name cannot be empty");
    }
    if name == "." || name == ".." {
        bail!("remote directory entry cannot be a traversal component: {name}");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("remote directory entry must be a single path component: {name:?}");
    }
    if name.contains('\0') {
        bail!("remote directory entry cannot contain NUL bytes");
    }
    Ok(())
}

/// Validate a slash-separated path that must stay relative to an already
/// selected remote root. This is used on public SyncPlan input as well as paths
/// generated from remote directory entries.
pub(crate) fn validate_remote_relative_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("remote relative path cannot be empty");
    }
    if path.starts_with('/') || path.starts_with('\\') {
        bail!("remote sync path must be relative: {path:?}");
    }
    if path.contains('\0') {
        bail!("remote sync path cannot contain NUL bytes");
    }

    for component in path.split('/') {
        validate_remote_child_name(component)?;
    }
    Ok(())
}

pub(crate) fn join_remote_under_root(root: &str, relative: &str) -> Result<String> {
    validate_remote_relative_path(relative)?;
    Ok(if root.is_empty() || root == "." {
        relative.to_owned()
    } else if root == "/" {
        format!("/{relative}")
    } else if root.ends_with('/') {
        format!("{root}{relative}")
    } else {
        format!("{root}/{relative}")
    })
}

pub(crate) fn local_path_from_remote_relative(relative: &str) -> Result<PathBuf> {
    validate_remote_relative_path(relative)?;
    let mut path = PathBuf::new();
    for component in relative.split('/') {
        let mut native = Path::new(component).components();
        match (native.next(), native.next()) {
            (Some(Component::Normal(_)), None) => path.push(component),
            _ => bail!(
                "sync path component is not a normal local filename on this platform: {component:?}"
            ),
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_nested_relative_paths() {
        validate_remote_relative_path("releases/2026/app.bin").unwrap();
        assert_eq!(
            join_remote_under_root("/srv/app", "releases/2026/app.bin").unwrap(),
            "/srv/app/releases/2026/app.bin"
        );
        assert_eq!(
            local_path_from_remote_relative("releases/2026/app.bin").unwrap(),
            PathBuf::from("releases").join("2026").join("app.bin")
        );
    }

    #[test]
    fn rejects_parent_current_absolute_and_empty_components() {
        for path in [
            "",
            ".",
            "..",
            "../etc/passwd",
            "a/../b",
            "a/./b",
            "/etc/passwd",
            "a//b",
            "a/",
            "\\server\\share",
            "a\\..\\b",
        ] {
            assert!(
                validate_remote_relative_path(path).is_err(),
                "path should be rejected: {path:?}"
            );
        }
    }

    #[test]
    fn rejects_server_supplied_multi_component_names() {
        for name in ["", ".", "..", "a/b", "a\\b", "bad\0name"] {
            assert!(validate_remote_child_name(name).is_err(), "{name:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_drive_relative_local_components() {
        assert!(local_path_from_remote_relative("C:escape").is_err());
    }
}
