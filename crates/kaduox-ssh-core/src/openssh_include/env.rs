use std::ffi::CStr;
#[cfg(windows)]
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
#[cfg(windows)]
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};

const HOST_NAME_BUFFER_BYTES: usize = 256;

#[derive(Debug)]
struct LocalHostnames {
    full: String,
    short: String,
}

#[cfg(windows)]
static WINSOCK_INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();

pub(super) fn expand_include_environment(value: &str, max_bytes: usize) -> Result<String> {
    expand_include_environment_with(value, max_bytes, |name| {
        std::env::var(name).with_context(|| {
            format!("OpenSSH Include environment variable ${{{name}}} is not available as UTF-8")
        })
    })
}

fn expand_include_environment_with<F>(
    value: &str,
    max_bytes: usize,
    mut lookup: F,
) -> Result<String>
where
    F: FnMut(&str) -> Result<String>,
{
    let mut output = String::with_capacity(value.len().min(max_bytes));
    let mut rest = value;
    let mut local_names: Option<LocalHostnames> = None;

    while let Some(start) = rest.find("${") {
        expand_percent_tokens(
            &mut output,
            &rest[..start],
            max_bytes,
            &mut local_names,
        )?;

        let variable_and_rest = &rest[start + 2..];
        let end = variable_and_rest
            .find('}')
            .context("OpenSSH Include contains an unterminated ${...} environment expansion")?;
        let name = &variable_and_rest[..end];
        if name.is_empty() {
            bail!("OpenSSH Include contains an empty environment variable name");
        }

        let value = lookup(name).with_context(|| {
            format!("failed to expand OpenSSH Include environment variable ${{{name}}}")
        })?;
        // Match OpenSSH's single-pass percent+dollar expansion: bytes produced
        // by an environment lookup are appended verbatim and are not rescanned
        // as percent tokens or nested ${...} expressions.
        push_bounded(&mut output, &value, max_bytes)?;
        rest = &variable_and_rest[end + 1..];
    }

    expand_percent_tokens(&mut output, rest, max_bytes, &mut local_names)?;
    Ok(output)
}

fn expand_percent_tokens(
    output: &mut String,
    value: &str,
    max_bytes: usize,
    local_names: &mut Option<LocalHostnames>,
) -> Result<()> {
    let mut rest = value;
    loop {
        let Some(index) = rest.find('%') else {
            push_bounded(output, rest, max_bytes)?;
            return Ok(());
        };
        push_bounded(output, &rest[..index], max_bytes)?;

        let after_percent = &rest[index + 1..];
        let Some(token) = after_percent.chars().next() else {
            bail!("OpenSSH Include ends with an incomplete percent token");
        };

        match token {
            '%' => push_bounded(output, "%", max_bytes)?,
            'l' | 'L' => {
                if local_names.is_none() {
                    *local_names = Some(query_local_hostnames()?);
                }
                let names = local_names
                    .as_ref()
                    .context("local hostname cache was not initialized")?;
                let replacement = if token == 'l' {
                    names.full.as_str()
                } else {
                    names.short.as_str()
                };
                push_bounded(output, replacement, max_bytes)?;
            }
            _ => bail!("OpenSSH Include percent token %{token} is not supported yet"),
        }

        rest = &after_percent[token.len_utf8()..];
    }
}

fn query_local_hostnames() -> Result<LocalHostnames> {
    let full = query_local_hostname()?;
    if full.is_empty() {
        bail!("local hostname is empty");
    }
    let short = full
        .split('.')
        .next()
        .unwrap_or(full.as_str())
        .to_owned();
    Ok(LocalHostnames { full, short })
}

fn hostname_from_buffer(buffer: &[c_char]) -> Result<String> {
    // Every caller reserves one untouched trailing NUL beyond the length passed
    // to the platform API, so CStr::from_ptr cannot read beyond `buffer` even
    // if the platform reports a truncated hostname without terminating it.
    let hostname = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    let bytes = hostname.to_bytes();
    if bytes.len() >= HOST_NAME_BUFFER_BYTES {
        bail!("local hostname exceeds the {HOST_NAME_BUFFER_BYTES}-byte safety limit");
    }
    if bytes.is_empty() {
        bail!("local hostname is empty");
    }
    let hostname = std::str::from_utf8(bytes)
        .context("local hostname used by OpenSSH Include is not valid UTF-8")?;
    Ok(hostname.to_owned())
}

#[cfg(not(windows))]
fn query_local_hostname() -> Result<String> {
    let mut buffer = [0 as c_char; HOST_NAME_BUFFER_BYTES + 1];
    let result = unsafe { system_gethostname(buffer.as_mut_ptr(), HOST_NAME_BUFFER_BYTES) };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .context("gethostname failed while expanding OpenSSH Include");
    }
    hostname_from_buffer(&buffer)
}

#[cfg(windows)]
fn query_local_hostname() -> Result<String> {
    ensure_winsock_started()?;

    let mut buffer = [0 as c_char; HOST_NAME_BUFFER_BYTES + 1];
    let result = unsafe {
        system_gethostname(
            buffer.as_mut_ptr(),
            c_int::try_from(HOST_NAME_BUFFER_BYTES).expect("hostname buffer length fits c_int"),
        )
    };
    if result != 0 {
        let error = unsafe { wsa_get_last_error() };
        bail!("gethostname failed while expanding OpenSSH Include with Winsock error {error}");
    }
    hostname_from_buffer(&buffer)
}

#[cfg(windows)]
fn ensure_winsock_started() -> Result<()> {
    match WINSOCK_INIT.get_or_init(|| {
        let mut startup = WsaStartupBuffer([0; 512]);
        let result = unsafe { wsa_startup(0x0202, startup.0.as_mut_ptr().cast()) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!("WSAStartup(2.2) failed with error {result}"))
        }
    }) {
        Ok(()) => Ok(()),
        Err(message) => bail!("failed to initialize Winsock for OpenSSH Include: {message}"),
    }
}

#[cfg(not(windows))]
unsafe extern "C" {
    #[link_name = "gethostname"]
    fn system_gethostname(name: *mut c_char, len: usize) -> c_int;
}

#[cfg(windows)]
#[repr(align(16))]
struct WsaStartupBuffer([u8; 512]);

#[cfg(windows)]
#[link(name = "ws2_32")]
unsafe extern "system" {
    #[link_name = "WSAStartup"]
    fn wsa_startup(version: u16, data: *mut c_void) -> c_int;
    #[link_name = "WSAGetLastError"]
    fn wsa_get_last_error() -> c_int;
    #[link_name = "gethostname"]
    fn system_gethostname(name: *mut c_char, len: c_int) -> c_int;
}

fn push_bounded(output: &mut String, value: &str, max_bytes: usize) -> Result<()> {
    let new_len = output
        .len()
        .checked_add(value.len())
        .context("OpenSSH Include environment/percent expansion byte accounting overflow")?;
    if new_len > max_bytes {
        bail!("expanded OpenSSH Include path exceeds the {max_bytes}-byte safety limit");
    }
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Result<String> {
        match name {
            "CONF_ROOT" => Ok("conf.d".to_owned()),
            "FILE" => Ok("prod.conf".to_owned()),
            "SPACED" => Ok("dir with spaces".to_owned()),
            "PERCENT" => Ok("literal%l.conf".to_owned()),
            "NESTED" => Ok("${FILE}".to_owned()),
            "A%B" => Ok("percent-name.conf".to_owned()),
            _ => bail!("missing test variable {name}"),
        }
    }

    #[test]
    fn expands_multiple_environment_references_without_rescanning_values() {
        assert_eq!(
            expand_include_environment_with("${CONF_ROOT}/${FILE}", 1024, lookup).unwrap(),
            "conf.d/prod.conf"
        );
        assert_eq!(
            expand_include_environment_with("${NESTED}", 1024, lookup).unwrap(),
            "${FILE}"
        );
    }

    #[test]
    fn preserves_plain_dollar_and_environment_value_characters() {
        assert_eq!(
            expand_include_environment_with("price$5/${SPACED}/${PERCENT}", 1024, lookup)
                .unwrap(),
            "price$5/dir with spaces/literal%l.conf"
        );
        assert_eq!(
            expand_include_environment_with("${A%B}", 1024, lookup).unwrap(),
            "percent-name.conf"
        );
    }

    #[test]
    fn literal_percent_escape_matches_openssh_without_rescanning_output() {
        assert_eq!(
            expand_include_environment_with("conf.d/100%%.conf", 1024, lookup).unwrap(),
            "conf.d/100%.conf"
        );
        assert_eq!(
            expand_include_environment_with("%%%%", 1024, lookup).unwrap(),
            "%%"
        );
        assert_eq!(
            expand_include_environment_with("%%l", 1024, lookup).unwrap(),
            "%l"
        );
        assert_eq!(
            expand_include_environment_with("${CONF_ROOT}/%%done", 1024, lookup).unwrap(),
            "conf.d/%done"
        );
    }

    #[test]
    fn local_hostname_tokens_match_native_hostname() {
        let names = query_local_hostnames().unwrap();
        assert_eq!(
            expand_include_environment_with("%l/%L", 1024, lookup).unwrap(),
            format!("{}/{}", names.full, names.short)
        );
    }

    #[test]
    fn named_or_incomplete_percent_tokens_remain_fail_closed() {
        for input in [
            "%h.conf",
            "${CONF_ROOT}/%n.conf",
            "%",
            "ok%%/%d",
            "%u.conf",
            "%i.conf",
        ] {
            assert!(
                expand_include_environment_with(input, 1024, lookup).is_err(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn malformed_or_missing_variables_fail_closed() {
        for input in ["${", "${}", "before-${MISSING}-after"] {
            assert!(
                expand_include_environment_with(input, 1024, lookup).is_err(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn expanded_size_is_bounded_after_percent_expansion() {
        assert!(expand_include_environment_with("${SPACED}", 3, lookup).is_err());
        assert_eq!(
            expand_include_environment_with("%%a", 2, lookup).unwrap(),
            "%a"
        );
        assert!(expand_include_environment_with("%%ab", 2, lookup).is_err());

        let names = query_local_hostnames().unwrap();
        assert!(
            expand_include_environment_with("%l", names.full.len() - 1, lookup).is_err()
        );
        assert_eq!(
            expand_include_environment_with("%l", names.full.len(), lookup).unwrap(),
            names.full.as_str()
        );
    }
}
