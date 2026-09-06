use std::ffi::c_void;
use std::fs;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr::{addr_of_mut, null, null_mut};

use anyhow::{Context, Result, bail};

type Handle = *mut c_void;
type Sid = *mut c_void;
type SidStorage = Vec<usize>;
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
const STANDARD_ALLOW_SID_OFFSET: usize = size_of::<AceHeader>() + size_of::<u32>();
const SID_HEADER_BYTES: usize = 8;

#[repr(C)]
struct Acl {
    _acl_revision: u8,
    _sbz1: u8,
    _acl_size: u16,
    ace_count: u16,
    _sbz2: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AceHeader {
    ace_type: u8,
    _ace_flags: u8,
    ace_size: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AccessAllowedAce {
    _header: AceHeader,
    mask: u32,
    sid_start: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SidHeader {
    _revision: u8,
    sub_authority_count: u8,
    _identifier_authority: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SidAndAttributes {
    sid: Sid,
    _attributes: u32,
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
            // SAFETY: GetSecurityInfo allocates the returned descriptor with
            // LocalAlloc and transfers ownership to the caller.
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
            // OpenProcessToken. It never contains the process pseudo-handle.
            unsafe {
                let _ = close_handle(self.0);
            }
        }
    }
}

pub(super) fn verify_open_file_acl(path: &Path, file: &fs::File) -> Result<()> {
    let user_sid_storage = current_user_sid().with_context(|| {
        format!(
            "failed to resolve the current Windows user SID for {}",
            path.display()
        )
    })?;
    let user_sid = sid_pointer(&user_sid_storage);
    let trusted_installer_storage = lookup_account_sid_optional("NT SERVICE\\TrustedInstaller");
    let trusted_installer_sid = trusted_installer_storage.as_ref().map(sid_pointer);

    let mut owner: Sid = null_mut();
    let mut dacl: *mut Acl = null_mut();
    let mut descriptor: SecurityDescriptor = null_mut();
    let status = unsafe {
        // SAFETY: file owns a live HANDLE for this call. The output owner/DACL
        // pointers are backed by descriptor until the guard below is dropped.
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
            "OpenSSH config {} has an untrusted Windows owner; expected the current user, \
             BUILTIN\\Administrators, LocalSystem, or TrustedInstaller",
            path.display()
        );
    }

    // A NULL DACL grants full access to everyone. An empty DACL is different:
    // it grants no access and remains valid.
    if dacl.is_null() || unsafe { is_valid_acl(dacl) } == 0 {
        bail!(
            "OpenSSH config {} has a missing, NULL, or invalid Windows DACL",
            path.display()
        );
    }

    let ace_count = unsafe { (*dacl).ace_count };
    for index in 0..u32::from(ace_count) {
        inspect_ace(
            path,
            dacl,
            index,
            user_sid,
            trusted_installer_sid,
        )?;
    }

    Ok(())
}

fn inspect_ace(
    path: &Path,
    dacl: *const Acl,
    index: u32,
    user_sid: Sid,
    trusted_installer_sid: Option<Sid>,
) -> Result<()> {
    let mut ace: *mut c_void = null_mut();
    if unsafe { get_ace(dacl, index, &mut ace) } == 0 || ace.is_null() {
        bail!(
            "failed to inspect Windows ACL entry {index} for OpenSSH config {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }

    let header = unsafe {
        // SAFETY: IsValidAcl succeeded before this helper is called and GetAce
        // returned an ACE pointer owned by that ACL.
        std::ptr::read_unaligned(ace.cast::<AceHeader>())
    };
    if header.ace_type != ACCESS_ALLOWED_ACE_TYPE {
        if is_advanced_allow_ace(header.ace_type) {
            bail!(
                "OpenSSH config {} uses unsupported advanced Windows allow ACE type {:#04x}; \
                 refusing to approximate its write permissions",
                path.display(),
                header.ace_type
            );
        }
        return Ok(());
    }

    let ace_size = usize::from(header.ace_size);
    if ace_size < STANDARD_ALLOW_SID_OFFSET + SID_HEADER_BYTES {
        bail!(
            "OpenSSH config {} contains a truncated Windows allow ACE",
            path.display()
        );
    }

    let allowed = unsafe {
        // SAFETY: the minimum ACE-size check above covers the fixed
        // ACCESS_ALLOWED_ACE fields read here.
        std::ptr::read_unaligned(ace.cast::<AccessAllowedAce>())
    };
    let sid = unsafe {
        // SAFETY: ACCESS_ALLOWED_ACE stores its variable-length SID at
        // SidStart. The fixed portion is covered by ace_size above.
        addr_of_mut!((*ace.cast::<AccessAllowedAce>()).sid_start).cast::<c_void>()
    };
    validate_sid_within_ace(path, sid, ace_size)?;

    if sid_is_trusted(sid, user_sid, trusted_installer_sid) {
        return Ok(());
    }
    if allowed.mask & SSH_SECURE_WRITE_MASK == 0 {
        // Win32-OpenSSH checks user config with read_ok=1, so an otherwise
        // untrusted principal may retain read-only access.
        return Ok(());
    }

    bail!(
        "OpenSSH config {} grants write-class Windows ACL rights to an untrusted principal",
        path.display()
    )
}

fn validate_sid_within_ace(path: &Path, sid: Sid, ace_size: usize) -> Result<()> {
    let sid_header = unsafe {
        // SAFETY: inspect_ace verified that the ACE contains at least the fixed
        // SID header before computing this pointer.
        std::ptr::read_unaligned(sid.cast::<SidHeader>())
    };
    let sub_authority_bytes = usize::from(sid_header.sub_authority_count)
        .checked_mul(size_of::<u32>())
        .context("Windows SID sub-authority length overflow")?;
    let sid_bytes = SID_HEADER_BYTES
        .checked_add(sub_authority_bytes)
        .context("Windows SID length overflow")?;
    let available = ace_size - STANDARD_ALLOW_SID_OFFSET;
    if sid_bytes > available || unsafe { is_valid_sid(sid) } == 0 {
        bail!(
            "OpenSSH config {} contains an invalid trustee SID in its Windows DACL",
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
        // SAFETY: callers validate SIDs before passing them here.
        equal_sid(sid, user_sid) != 0
            || is_well_known_sid(sid, WIN_BUILTIN_ADMINISTRATORS_SID) != 0
            || is_well_known_sid(sid, WIN_LOCAL_SYSTEM_SID) != 0
            || trusted_installer_sid.is_some_and(|trusted| equal_sid(sid, trusted) != 0)
    }
}

fn current_user_sid() -> Result<SidStorage> {
    let mut raw_token: Handle = null_mut();
    if unsafe { open_process_token(get_current_process(), TOKEN_QUERY, &mut raw_token) } == 0 {
        bail!(
            "OpenProcessToken failed: {}",
            std::io::Error::last_os_error()
        );
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

    let mut information = aligned_storage(required);
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
        // SAFETY: aligned_storage provides at least pointer alignment, and
        // GetTokenInformation wrote TOKEN_USER at the beginning of the buffer.
        std::ptr::read(information.as_ptr().cast::<TokenUserInfo>())
    };
    copy_valid_sid(token_user.user.sid).context("current token contains an invalid user SID")
}

fn copy_valid_sid(sid: Sid) -> Result<SidStorage> {
    if sid.is_null() || unsafe { is_valid_sid(sid) } == 0 {
        bail!("invalid Windows SID");
    }
    let len = unsafe { get_length_sid(sid) };
    if len == 0 {
        bail!("Windows SID has zero length");
    }

    let mut copy = aligned_storage(len);
    unsafe {
        // SAFETY: GetLengthSid returned the byte length of a SID accepted by
        // IsValidSid, and copy has at least that many writable bytes.
        std::ptr::copy_nonoverlapping(
            sid.cast::<u8>(),
            copy.as_mut_ptr().cast::<u8>(),
            len as usize,
        );
    }
    Ok(copy)
}

fn lookup_account_sid_optional(account: &str) -> Option<SidStorage> {
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

    let error = std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or_default() as u32;
    if error == ERROR_NONE_MAPPED {
        return None;
    }
    if error != ERROR_INSUFFICIENT_BUFFER || sid_len == 0 {
        return None;
    }

    let mut sid = aligned_storage(sid_len);
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
    if ok == 0 || unsafe { is_valid_sid(sid_pointer(&sid)) } == 0 {
        return None;
    }
    Some(sid)
}

fn aligned_storage(bytes: u32) -> SidStorage {
    let words = (bytes as usize).div_ceil(size_of::<usize>());
    vec![0usize; words]
}

fn sid_pointer(storage: &SidStorage) -> Sid {
    storage.as_ptr().cast_mut().cast::<c_void>()
}
