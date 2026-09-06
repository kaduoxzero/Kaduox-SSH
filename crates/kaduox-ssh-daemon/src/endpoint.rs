use std::io;

#[cfg(unix)]
mod platform {
    use std::fs;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};
    use tokio::net::{UnixListener, UnixStream};

    pub struct ServerEndpoint {
        listener: UnixListener,
        path: PathBuf,
        owner_uid: u32,
    }

    impl ServerEndpoint {
        pub fn bind_default() -> Result<Self> {
            Self::bind(default_endpoint_path()?)
        }

        pub fn bind(path: impl Into<PathBuf>) -> Result<Self> {
            let path = path.into();
            let parent = path
                .parent()
                .context("daemon socket path has no parent directory")?;
            ensure_private_directory(parent)?;
            let owner_uid = fs::metadata(parent)
                .with_context(|| format!("failed to stat daemon directory {}", parent.display()))?
                .uid();

            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        bail!("refusing symlink daemon socket path {}", path.display());
                    }
                    if !metadata.file_type().is_socket() {
                        bail!(
                            "existing daemon endpoint {} is not a Unix socket",
                            path.display()
                        );
                    }
                    if metadata.uid() != owner_uid {
                        bail!(
                            "existing daemon socket {} has a different owner",
                            path.display()
                        );
                    }
                    match StdUnixStream::connect(&path) {
                        Ok(_) => bail!("kssh-daemon is already running at {}", path.display()),
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::ConnectionRefused
                                    | std::io::ErrorKind::NotFound
                            ) =>
                        {
                            fs::remove_file(&path).with_context(|| {
                                format!("failed to remove stale daemon socket {}", path.display())
                            })?;
                        }
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!(
                                    "failed to determine whether daemon socket {} is active",
                                    path.display()
                                )
                            });
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to inspect daemon socket {}", path.display())
                    });
                }
            }

            let listener = UnixListener::bind(&path)
                .with_context(|| format!("failed to bind daemon socket {}", path.display()))?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).with_context(|| {
                format!(
                    "failed to set daemon socket {} mode to 0600",
                    path.display()
                )
            })?;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.uid() != owner_uid || metadata.permissions().mode() & 0o077 != 0 {
                let _ = fs::remove_file(&path);
                bail!(
                    "daemon socket {} failed private ownership/mode validation",
                    path.display()
                );
            }

            Ok(Self {
                listener,
                path,
                owner_uid,
            })
        }

        pub async fn accept(&self) -> Result<UnixStream> {
            loop {
                let (stream, _) = self
                    .listener
                    .accept()
                    .await
                    .context("daemon socket accept failed")?;
                let credentials = stream
                    .peer_cred()
                    .context("failed to read Unix peer credentials")?;
                if credentials.uid() != self.owner_uid {
                    drop(stream);
                    continue;
                }
                return Ok(stream);
            }
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for ServerEndpoint {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    pub async fn connect_default() -> Result<UnixStream> {
        connect(default_endpoint_path()?).await
    }

    pub async fn connect(path: impl AsRef<Path>) -> Result<UnixStream> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .context("daemon socket path has no parent directory")?;
        let parent_metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("failed to inspect daemon directory {}", parent.display()))?;
        if parent_metadata.file_type().is_symlink()
            || !parent_metadata.is_dir()
            || parent_metadata.permissions().mode() & 0o077 != 0
        {
            bail!("daemon directory {} is not private", parent.display());
        }

        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("failed to inspect daemon socket {}", path.display()))?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_socket()
            || metadata.permissions().mode() & 0o077 != 0
        {
            bail!(
                "daemon endpoint {} is not a private Unix socket",
                path.display()
            );
        }
        if metadata.uid() != parent_metadata.uid() {
            bail!(
                "daemon endpoint {} owner differs from its private directory",
                path.display()
            );
        }

        let stream = UnixStream::connect(path)
            .await
            .with_context(|| format!("failed to connect daemon socket {}", path.display()))?;
        let credentials = stream
            .peer_cred()
            .context("failed to read daemon peer credentials")?;
        if credentials.uid() != metadata.uid() {
            bail!("daemon peer UID does not match socket owner");
        }
        Ok(stream)
    }

    pub fn default_endpoint_path() -> Result<PathBuf> {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            if !runtime.is_empty() {
                return Ok(PathBuf::from(runtime)
                    .join("kaduox-ssh")
                    .join("daemon.sock"));
            }
        }

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set; cannot determine daemon socket path")?;
        #[cfg(target_os = "macos")]
        {
            return Ok(home
                .join("Library")
                .join("Caches")
                .join("Kaduox-SSH")
                .join("daemon.sock"));
        }
        #[cfg(not(target_os = "macos"))]
        Ok(home.join(".cache").join("kaduox-ssh").join("daemon.sock"))
    }

    fn ensure_private_directory(path: &Path) -> Result<()> {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create daemon directory {}", path.display()))?;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!(
                "daemon directory {} is not a real directory",
                path.display()
            );
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).with_context(|| {
            format!(
                "failed to set daemon directory {} mode to 0700",
                path.display()
            )
        })?;
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr;
    use std::time::Duration;

    use anyhow::{Context, Result, bail};
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_BUSY, GetLastError, HANDLE,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Pipes::{
        GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    };
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, GetCurrentProcess, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const PIPE_NAME: &str = r"\\.\pipe\kaduox-ssh-daemon-v1";
    const SINGLETON_NAME: &str = r"Local\KaduoxSSHDaemon-v1";

    pub struct ServerEndpoint {
        singleton: HANDLE,
    }

    impl ServerEndpoint {
        pub fn bind_default() -> Result<Self> {
            let name = wide_null(SINGLETON_NAME);
            let singleton = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
            if singleton.is_null() {
                bail!("failed to create kssh-daemon singleton mutex");
            }
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    let _ = CloseHandle(singleton);
                }
                bail!("kssh-daemon is already running for this Windows session");
            }
            Ok(Self { singleton })
        }

        pub async fn accept(&self) -> Result<NamedPipeServer> {
            loop {
                let server = ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(PIPE_NAME)
                    .context("failed to create local daemon named pipe")?;
                server
                    .connect()
                    .await
                    .context("daemon named-pipe accept failed")?;
                let mut pid = 0_u32;
                let handle = server.as_raw_handle() as HANDLE;
                let ok = unsafe { GetNamedPipeClientProcessId(handle, &mut pid) };
                if ok == 0 || pid == 0 || !process_is_current_user(pid)? {
                    drop(server);
                    continue;
                }
                return Ok(server);
            }
        }
    }

    impl Drop for ServerEndpoint {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.singleton);
            }
        }
    }

    pub async fn connect_default() -> Result<NamedPipeClient> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match ClientOptions::new().open(PIPE_NAME) {
                Ok(client) => {
                    let mut pid = 0_u32;
                    let handle = client.as_raw_handle() as HANDLE;
                    let ok = unsafe { GetNamedPipeServerProcessId(handle, &mut pid) };
                    if ok == 0 || pid == 0 || !process_is_current_user(pid)? {
                        bail!("daemon named-pipe server identity does not match current user");
                    }
                    return Ok(client);
                }
                Err(error)
                    if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => return Err(error).context("failed to connect daemon named pipe"),
            }
        }
    }

    pub fn default_endpoint_path() -> Result<std::path::PathBuf> {
        Ok(std::path::PathBuf::from(PIPE_NAME))
    }

    fn process_is_current_user(pid: u32) -> Result<bool> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Ok(false);
        }
        let result = (|| -> Result<bool> {
            let client_token = open_process_token(process)?;
            let current_token = open_process_token(unsafe { GetCurrentProcess() })?;
            let client_info = token_user_info(client_token)?;
            let current_info = token_user_info(current_token)?;
            unsafe {
                let _ = CloseHandle(client_token);
                let _ = CloseHandle(current_token);
            }
            let client_user = unsafe { &*(client_info.as_ptr() as *const TOKEN_USER) };
            let current_user = unsafe { &*(current_info.as_ptr() as *const TOKEN_USER) };
            Ok(unsafe { EqualSid(client_user.User.Sid, current_user.User.Sid) } != 0)
        })();
        unsafe {
            let _ = CloseHandle(process);
        }
        result
    }

    fn open_process_token(process: HANDLE) -> Result<HANDLE> {
        let mut token: HANDLE = ptr::null_mut::<c_void>();
        let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
        if ok == 0 || token.is_null() {
            bail!("failed to open process token for daemon peer verification");
        }
        Ok(token)
    }

    fn token_user_info(token: HANDLE) -> Result<Vec<u8>> {
        let mut required = 0_u32;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required);
        }
        if required == 0 || required > 64 * 1024 {
            bail!("invalid token-user information size {required}");
        }
        let mut buffer = vec![0_u8; required as usize];
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        };
        if ok == 0 {
            bail!("failed to read process token user for daemon peer verification");
        }
        Ok(buffer)
    }

    fn wide_null(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use anyhow::{Result, bail};

    pub struct ServerEndpoint;

    impl ServerEndpoint {
        pub fn bind_default() -> Result<Self> {
            bail!("kssh-daemon IPC is not implemented on this platform")
        }
    }

    pub async fn connect_default() -> Result<()> {
        bail!("kssh-daemon IPC is not implemented on this platform")
    }

    pub fn default_endpoint_path() -> Result<std::path::PathBuf> {
        bail!("kssh-daemon IPC is not implemented on this platform")
    }
}

pub use platform::*;

pub fn is_endpoint_unavailable(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .any(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
            )
        })
}
