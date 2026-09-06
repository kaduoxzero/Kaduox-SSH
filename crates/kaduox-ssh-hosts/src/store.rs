use std::cmp::Reverse;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::codec;
use crate::model::{HostDatabase, HostRecord, JumpChain, JumpHop, StoredAuthMethod};

const MAX_STORE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct HostStore {
    path: PathBuf,
    database: HostDatabase,
}

impl HostStore {
    pub fn open_default() -> Result<Self> {
        Self::open(default_store_path()?)
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let database = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    bail!("host database {} must not be a symlink", path.display());
                }
                validate_private_file(&path)?;
                if !metadata.is_file() {
                    bail!("host database {} is not a regular file", path.display());
                }
                if metadata.len() > MAX_STORE_BYTES {
                    bail!(
                        "host database {} exceeds {} bytes",
                        path.display(),
                        MAX_STORE_BYTES
                    );
                }
                let mut input = String::with_capacity(metadata.len() as usize);
                File::open(&path)
                    .with_context(|| format!("failed to open host database {}", path.display()))?
                    .read_to_string(&mut input)
                    .with_context(|| {
                        format!("failed to read UTF-8 host database {}", path.display())
                    })?;
                codec::decode(&input)
                    .with_context(|| format!("failed to parse host database {}", path.display()))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HostDatabase::default(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect host database {}", path.display()));
            }
        };
        Ok(Self { path, database })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn database(&self) -> &HostDatabase {
        &self.database
    }

    pub fn host(&self, alias: &str) -> Option<&HostRecord> {
        self.database.hosts.get(alias)
    }

    pub fn chain(&self, name: &str) -> Option<&JumpChain> {
        self.database.chains.get(name)
    }

    pub fn hosts_recent_first(&self) -> Vec<&HostRecord> {
        let mut hosts = self.database.hosts.values().collect::<Vec<_>>();
        hosts.sort_by_key(|host| {
            (
                Reverse(host.stats.last_connected_unix.unwrap_or(0)),
                Reverse(host.stats.connection_count),
                host.alias.as_str(),
            )
        });
        hosts
    }

    pub fn upsert_host(&mut self, host: HostRecord) -> Result<Option<HostRecord>> {
        host.validate()?;
        if let Some(chain) = &host.jump_chain {
            if !self.database.chains.contains_key(chain) {
                bail!("host {} references missing jump chain {chain}", host.alias);
            }
        }
        let alias = host.alias.clone();
        let previous = self.database.hosts.insert(alias.clone(), host);
        if let Err(error) = self.database.validate() {
            match previous.clone() {
                Some(old) => {
                    self.database.hosts.insert(alias, old);
                }
                None => {
                    self.database.hosts.remove(&alias);
                }
            }
            return Err(error);
        }
        Ok(previous)
    }

    pub fn insert_host(&mut self, host: HostRecord) -> Result<()> {
        if self.database.hosts.contains_key(&host.alias) {
            bail!("host alias {:?} already exists", host.alias);
        }
        let alias = host.alias.clone();
        host.validate()?;
        self.database.hosts.insert(alias.clone(), host);
        if let Err(error) = self.database.validate() {
            self.database.hosts.remove(&alias);
            return Err(error);
        }
        Ok(())
    }

    pub fn remove_host(&mut self, alias: &str) -> Result<HostRecord> {
        for chain in self.database.chains.values() {
            if chain
                .hops
                .iter()
                .any(|hop| matches!(hop, JumpHop::Host(value) if value == alias))
            {
                bail!(
                    "cannot remove host {alias}; jump chain {} references it",
                    chain.name
                );
            }
        }
        self.database
            .hosts
            .remove(alias)
            .with_context(|| format!("host alias {alias:?} does not exist"))
    }

    pub fn upsert_chain(&mut self, chain: JumpChain) -> Result<Option<JumpChain>> {
        chain.validate()?;
        let name = chain.name.clone();
        let previous = self.database.chains.insert(name.clone(), chain);
        if let Err(error) = self.database.validate() {
            match previous.clone() {
                Some(old) => {
                    self.database.chains.insert(name, old);
                }
                None => {
                    self.database.chains.remove(&name);
                }
            }
            return Err(error);
        }
        Ok(previous)
    }

    pub fn insert_chain(&mut self, chain: JumpChain) -> Result<()> {
        if self.database.chains.contains_key(&chain.name) {
            bail!("jump chain {:?} already exists", chain.name);
        }
        let name = chain.name.clone();
        self.database.chains.insert(name.clone(), chain);
        if let Err(error) = self.database.validate() {
            self.database.chains.remove(&name);
            return Err(error);
        }
        Ok(())
    }

    pub fn remove_chain(&mut self, name: &str) -> Result<JumpChain> {
        if let Some(host) = self
            .database
            .hosts
            .values()
            .find(|host| host.jump_chain.as_deref() == Some(name))
        {
            bail!(
                "cannot remove jump chain {name}; host {} is bound to it",
                host.alias
            );
        }
        self.database
            .chains
            .remove(name)
            .with_context(|| format!("jump chain {name:?} does not exist"))
    }

    pub fn record_success(&mut self, alias: &str, method: StoredAuthMethod) -> Result<()> {
        let host = self
            .database
            .hosts
            .get_mut(alias)
            .with_context(|| format!("cannot update statistics for unknown host {alias:?}"))?;
        host.stats.connection_count = host
            .stats
            .connection_count
            .checked_add(1)
            .context("host connection counter overflow")?;
        host.stats.last_connected_unix = Some(now_unix()?);
        host.stats.last_auth_method = Some(method);
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        self.database.validate()?;
        let encoded = codec::encode(&self.database)?;
        if encoded.len() as u64 > MAX_STORE_BYTES {
            bail!("encoded host database exceeds {MAX_STORE_BYTES} bytes");
        }
        atomic_write_private(&self.path, encoded.as_bytes())
    }
}

pub fn default_store_path() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .context("APPDATA is not set; cannot determine Kaduox-SSH config directory")?;
        Ok(base.join("Kaduox-SSH").join("hosts.toml"))
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set; cannot determine Kaduox-SSH config directory")?;
        return Ok(home
            .join("Library")
            .join("Application Support")
            .join("Kaduox-SSH")
            .join("hosts.toml"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("kaduox-ssh").join("hosts.toml"));
            }
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set; cannot determine Kaduox-SSH config directory")?;
        Ok(home.join(".config").join("kaduox-ssh").join("hosts.toml"))
    }
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("host database path has no parent directory")?;
    ensure_private_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                bail!("host database {} must not be a symlink", path.display());
            }
            validate_private_file(path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect host database {}", path.display()));
        }
    }

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("host database file name is not valid UTF-8")?;
    let temp = parent.join(format!(
        ".{file_name}.kaduox-{}.{}.tmp",
        std::process::id(),
        now_unix_nanos()?
    ));

    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .with_context(|| format!("failed to create staging file {}", temp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write staging file {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync staging file {}", temp.display()))?;
        drop(file);
        set_private_file_permissions(&temp)?;

        // Staging and destination are siblings. The platform rename/replace
        // operation either publishes the complete new database or leaves the old
        // database intact; no partially-written final file is exposed.
        fs::rename(&temp, path).with_context(|| {
            format!(
                "failed to atomically publish host database {} -> {}",
                temp.display(),
                path.display()
            )
        })?;
        set_private_file_permissions(path)?;

        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("failed to fsync config directory {}", parent.display()))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create config directory {}", path.display()))?;
    reject_symlink(path, "config directory")?;
    if !fs::metadata(path)?.is_dir() {
        bail!("config path {} is not a directory", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).with_context(|| {
            format!("failed to set config directory {} mode to 0700", path.display())
        })?;
    }
    Ok(())
}

fn reject_symlink(path: &Path, description: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {description} {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("{description} {} must not be a symlink", path.display());
    }
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
            format!("failed to set host database {} mode to 0600", path.display())
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn validate_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            bail!(
                "host database {} has unsafe mode {:03o}; expected 0600 (run chmod 600)",
                path.display(),
                mode
            );
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn now_unix() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

fn now_unix_nanos() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "kaduox-host-store-test-{}-{}-{name}",
                std::process::id(),
                now_unix_nanos().unwrap()
            ))
            .join("hosts.toml")
    }

    #[test]
    fn save_and_reload_preserves_host() {
        let path = temp_store_path("roundtrip");
        let mut store = HostStore::open(&path).unwrap();
        store
            .insert_host(HostRecord::new("prod", "10.0.0.1", "deploy"))
            .unwrap();
        store.save().unwrap();
        let loaded = HostStore::open(&path).unwrap();
        assert_eq!(loaded.host("prod").unwrap().address, "10.0.0.1");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn statistics_increment_checked() {
        let path = temp_store_path("stats");
        let mut store = HostStore::open(path).unwrap();
        store
            .insert_host(HostRecord::new("prod", "10.0.0.1", "deploy"))
            .unwrap();
        store.record_success("prod", StoredAuthMethod::Agent).unwrap();
        let host = store.host("prod").unwrap();
        assert_eq!(host.stats.connection_count, 1);
        assert_eq!(host.stats.last_auth_method, Some(StoredAuthMethod::Agent));
    }

    #[test]
    fn failed_first_upsert_rolls_back_inserted_alias() {
        let path = temp_store_path("rollback");
        let mut store = HostStore::open(path).unwrap();
        let invalid = HostRecord {
            jump_chain: Some("missing".into()),
            ..HostRecord::new("prod", "10.0.0.1", "deploy")
        };
        assert!(store.upsert_host(invalid).is_err());
        assert!(store.host("prod").is_none());
    }
}
