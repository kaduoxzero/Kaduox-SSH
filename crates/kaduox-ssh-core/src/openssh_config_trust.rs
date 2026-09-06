use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

const MAX_CONFIG_FILE_BYTES: usize = 4 * 1024 * 1024;

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

#[cfg(windows)]
mod windows_acl {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{addr_of_mut, null, null_mut};

    type Handle = *mut c_void;
    type Sid = *mut c_void;
    type SecurityDescriptor = *mut c_void;

    const SE_FILE_OBJECT: u32 = 1;
    const OWNER_SECURITY_INFORMATION: u32 = 0x0000_0001;
    const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_USER: u32 = 1;

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0x00;
    const ACCESS_ALLOWED_COMPOUND_ACE_TYPE: u8 = 0x04;
    const ACCESS_ALLOWED_OBJECT_ACE_TYPE: u8 = 0x05;
    const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 0x09;
    const ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE: u8 = 0x0b;

    const WIN_LOCAL_SYSTEM_SID: i32 = 22;
    const WIN_BUILTIN_ADMINISTRATORS_SID: i32 = 26;

    const FILE_WRITE_DATA: u32 = 0x0000_0002;
    const FILE_APPEND_DATA: u32 = 0x0000_0004;
    const FILE_WRITE_EA: u32 = 0x0000_0010;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
    const DELETE: u32 = 0x0001_0000;
    const WRITE_DAC: u32 = 0x0004_0000;
    const WRITE_OWNER: u32 = 0x0008_0000;
    const SSH_SECURE_WRITE_MASK: u32 = FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER;

    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const ERROR_NONE_MAPPED: u32 = 1332;

    #[repr(C)]
    struct Acl {
        acl_revision: u8,
        sbz1: u8,
        acl_size: u16,
        ace_count: u16,
        sbz2: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AceHeader {
        ace_type: u8,
        ace_flags: u8,
        ace_size: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AccessAllowedAce {
        header: AceHeader,
        mask: u32,
        sid_start: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SidAndAttributes {
        sid: Sid,
        attributes: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TokenUserInfo {
        user: SidAndAttributes,
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        #[link_name = "GetSecurityInfo"]
        fn get_security_info(
            handle: Handle,
            object_type: u32,
            security_info: u32,
            owner: *mut Sid,
            group: *mut Sid,
            dacl: *mut *mut Acl,
            sacl: *mut *mut Acl,
            security_descriptor: *mut SecurityDescriptor,
        ) -> u32;
        #[link_name = "IsValidSid"]
        fn is_valid_sid(sid: Sid) -> i32;
        #[link_name = "IsValidAcl"]
        fn is_valid_acl(acl: *const Acl) -> i32;
        #[link_name = "IsWellKnownSid"]
        fn is_well_known_sid(sid: Sid, sid_type: i32) -> i32;
        #[link_name = "EqualSid"]
        fn equal_sid(first: Sid, second: Sid) -> i32;
        #[link_name = "GetAce"]
        fn get_ace(acl: *const Acl, index: u32, ace: *mut *mut c_void) -> i32;
        #[link_name = "OpenProcessToken"]
        fn open_process_token(process: Handle, access: u32, token: *mut Handle) -> i32;
        #[link_name = "GetTokenInformation"]
        fn get_token_information(
            token: Handle,
            class: u32,
            information: *mut c_void,
            information_len: u32,
            return_len: *mut u32,
        ) -> i32;
        #[link_name = "GetLengthSid"]
        fn get_length_sid(sid: Sid) -> u32;
        #[link_name = "LookupAccountNameW"]
        fn lookup_account_name_w(
            system_name: *const u16,
            account_name: *const u16,
            sid: Sid,
            sid_len: *mut u32,
            referenced_domain_name: *mut u16,
            domain_name_len: *mut u32,
            sid_name_use: *mut i32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "GetCurrentProcess"]
        fn get_current_process() -> Handle;
        #[link_name = "CloseHandle"]
        fn close_handle(handle: Handle) -> i32;
        #[link_name = "LocalFree"]
        fn local_free(memory: *mut c_void) -> *mut c_void;
    }

    struct LocalSecurityDescriptor(SecurityDescriptor);

    impl Drop for LocalSecurityDescriptor {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: GetSecurityInfo allocates the returned security
                // descriptor with LocalAlloc and transfers ownership to the
                // caller, which must release it with LocalFree.
                unsafe {
                    let _ = local_free(self.0);
                }
            }
        }
    }

    struct TokenHandle(Handle);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: this wrapper owns the real token handle returned by
                // OpenProcessToken. The pseudo process handle is never stored
                // here and therefore is never closed.
                unsafe {
                    let _ = close_handle(self.0);
                }
            }
        }
    }

    pub(super) fn verify_open_file_acl(path: &Path, file: &fs::File) -> Result<()> {
        let user_sid_storage = current_user_sid()
            .with_context(|| format!("failed to resolve the current Windows user SID for {}", path.display()))?;
        let user_sid = user_sid_storage.as_ptr().cast_mut().cast::<c_void>();
        let trusted_installer_sid_storage = lookup_account_sid_optional("NT SERVICE\\TrustedInstaller");
        let trusted_installer_sid = trusted_installer_sid_storage
            .as_ref()
            .map(|sid| sid.as_ptr().cast_mut().cast::<c_void>());

        let mut owner: Sid = null_mut();
        let mut dacl: *mut Acl = null_mut();
        let mut descriptor: SecurityDescriptor = null_mut();
        let status = unsafe {
            // SAFETY: file is an open std::fs::File whose RawHandle remains
            // valid for this call. Output pointers refer to memory owned by
            // the returned security descriptor and remain live until the
            // LocalSecurityDescriptor guard is dropped.
            get_security_info(
                file.as_raw_handle().cast::<c_void>(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            bail!(
                "failed to read Windows owner/DACL for OpenSSH config {}: {}",
                path.display(),
                std::io::Error::from_raw_os_error(status as i32)
            );
        }
        let _descriptor = LocalSecurityDescriptor(descriptor);

        if owner.is_null() || unsafe { is_valid_sid(owner) } == 0 {
            bail!(
                "OpenSSH config {} has a missing or invalid Windows owner SID",
                path.display()
            );
        }
        if !sid_is_trusted(owner, user_sid, trusted_installer_sid) {
            bail!(
                "OpenSSH config {} has an untrusted Windows owner; expected the current user, BUILTIN\\Administrators, LocalSystem, or TrustedInstaller",
                path.display()
            );
        }

        // A NULL DACL grants full access to everyone. Treat it as insecure
        // rather than confusing it with an empty DACL, which grants no access.
        if dacl.is_null() || unsafe { is_valid_acl(dacl) } == 0 {
            bail!(
                "OpenSSH config {} has a missing, NULL, or invalid Windows DACL",
                path.display()
            );
        }

        let ace_count = unsafe { (*dacl).ace_count };
        for index in 0..u32::from(ace_count) {
            let mut ace: *mut c_void = null_mut();
            if unsafe { get_ace(dacl, index, &mut ace) } == 0 || ace.is_null() {
                bail!(
                    "failed to inspect Windows ACL entry {index} for OpenSSH config {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                );
            }

            let header = unsafe {
                // SAFETY: IsValidAcl succeeded and GetAce returned an ACE
                // pointer owned by that ACL. read_unaligned avoids assuming a
                // stronger alignment than the ACL format guarantees.
                std::ptr::read_unaligned(ace.cast::<AceHeader>())
            };
            if header.ace_type != ACCESS_ALLOWED_ACE_TYPE {
                if is_advanced_allow_ace(header.ace_type) {
                    bail!(
                        "OpenSSH config {} uses unsupported advanced Windows allow ACE type {:#04x}; refusing to approximate its write permissions",
                        path.display(),
                        header.ace_type
                    );
                }
                continue;
            }
            if usize::from(header.ace_size) < std::mem::size_of::<AccessAllowedAce>() {
                bail!(
                    "OpenSSH config {} contains a truncated Windows allow ACE",
                    path.display()
                );
            }

            let allowed = unsafe { std::ptr::read_unaligned(ace.cast::<AccessAllowedAce>()) };
            let sid = unsafe {
                // SAFETY: a standard ACCESS_ALLOWED_ACE stores the variable
                // length SID beginning at SidStart. The ACL and ACE sizes were
                // validated by Windows before this pointer is used.
                addr_of_mut!((*ace.cast::<AccessAllowedAce>()).sid_start).cast::<c_void>()
            };
            if unsafe { is_valid_sid(sid) } == 0 {
                bail!(
                    "OpenSSH config {} contains an invalid trustee SID in its Windows DACL",
                    path.display()
                );
            }

            if sid_is_trusted(sid, user_sid, trusted_installer_sid) {
                continue;
            }
            if allowed.mask & SSH_SECURE_WRITE_MASK == 0 {
                // Win32-OpenSSH checks user config with read_ok=1, so an
                // otherwise-untrusted principal may retain read-only access.
                continue;
            }

            bail!(
                "OpenSSH config {} grants write-class Windows ACL rights to an untrusted principal",
                path.display()
            );
        }

        Ok(())
    }

    fn is_advanced_allow_ace(ace_type: u8) -> bool {
        matches!(
            ace_type,
            ACCESS_ALLOWED_COMPOUND_ACE_TYPE
                | ACCESS_ALLOWED_OBJECT_ACE_TYPE
                | ACCESS_ALLOWED_CALLBACK_ACE_TYPE
                | ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE
        )
    }

    fn sid_is_trusted(sid: Sid, user_sid: Sid, trusted_installer_sid: Option<Sid>) -> bool {
        unsafe {
            // SAFETY: all SIDs are validated before this helper is called.
            equal_sid(sid, user_sid) != 0
                || is_well_known_sid(sid, WIN_BUILTIN_ADMINISTRATORS_SID) != 0
                || is_well_known_sid(sid, WIN_LOCAL_SYSTEM_SID) != 0
                || trusted_installer_sid.is_some_and(|trusted| equal_sid(sid, trusted) != 0)
        }
    }

    fn current_user_sid() -> Result<Vec<u8>> {
        let mut raw_token: Handle = null_mut();
        if unsafe { open_process_token(get_current_process(), TOKEN_QUERY, &mut raw_token) } == 0 {
            bail!("OpenProcessToken failed: {}", std::io::Error::last_os_error());
        }
        let token = TokenHandle(raw_token);

        let mut required = 0u32;
        unsafe {
            let _ = get_token_information(token.0, TOKEN_USER, null_mut(), 0, &mut required);
        }
        if required == 0 {
            bail!(
                "GetTokenInformation(TOKEN_USER) did not report a buffer size: {}",
                std::io::Error::last_os_error()
            );
        }

        let mut information = vec![0u8; required as usize];
        if unsafe {
            get_token_information(
                token.0,
                TOKEN_USER,
                information.as_mut_ptr().cast::<c_void>(),
                required,
                &mut required,
            )
        } == 0
        {
            bail!(
                "GetTokenInformation(TOKEN_USER) failed: {}",
                std::io::Error::last_os_error()
            );
        }

        let token_user = unsafe {
            // SAFETY: GetTokenInformation wrote a TOKEN_USER structure at the
            // start of the supplied buffer. read_unaligned avoids depending on
            // Vec<u8>'s alignment.
            std::ptr::read_unaligned(information.as_ptr().cast::<TokenUserInfo>())
        };
        copy_valid_sid(token_user.user.sid).context("current token contains an invalid user SID")
    }

    fn copy_valid_sid(sid: Sid) -> Result<Vec<u8>> {
        if sid.is_null() || unsafe { is_valid_sid(sid) } == 0 {
            bail!("invalid Windows SID");
        }
        let len = unsafe { get_length_sid(sid) } as usize;
        if len == 0 {
            bail!("Windows SID has zero length");
        }
        let mut copy = vec![0u8; len];
        unsafe {
            // SAFETY: GetLengthSid returned the byte length of a SID already
            // accepted by IsValidSid, and copy has exactly that capacity.
            std::ptr::copy_nonoverlapping(sid.cast::<u8>(), copy.as_mut_ptr(), len);
        }
        Ok(copy)
    }

    fn lookup_account_sid_optional(account: &str) -> Option<Vec<u8>> {
        let account: Vec<u16> = account.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sid_len = 0u32;
        let mut domain_len = 0u32;
        let mut sid_name_use = 0i32;
        let first = unsafe {
            lookup_account_name_w(
                null(),
                account.as_ptr(),
                null_mut(),
                &mut sid_len,
                null_mut(),
                &mut domain_len,
                &mut sid_name_use,
            )
        };
        if first != 0 {
            return None;
        }
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or_default() as u32;
        if error == ERROR_NONE_MAPPED {
            return None;
        }
        if error != ERROR_INSUFFICIENT_BUFFER || sid_len == 0 {
            return None;
        }

        let mut sid = vec![0u8; sid_len as usize];
        let mut domain = vec![0u16; domain_len.max(1) as usize];
        let mut domain_capacity = domain.len() as u32;
        let ok = unsafe {
            lookup_account_name_w(
                null(),
                account.as_ptr(),
                sid.as_mut_ptr().cast::<c_void>(),
                &mut sid_len,
                domain.as_mut_ptr(),
                &mut domain_capacity,
                &mut sid_name_use,
            )
        };
        if ok == 0 || unsafe { is_valid_sid(sid.as_mut_ptr().cast::<c_void>()) } == 0 {
            return None;
        }
        sid.truncate(sid_len as usize);
        Some(sid)
    }
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

        grant_builtin_users(&config, "(W)");
        let error = read_user_config_file(&config, &home).unwrap_err().to_string();
        assert!(error.contains("write-class Windows ACL rights"), "{error}");
        fs::remove_dir_all(home).unwrap();
    }
}
