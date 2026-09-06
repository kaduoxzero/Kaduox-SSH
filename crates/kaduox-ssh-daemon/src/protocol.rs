use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::Authentication;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u16 = 2;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_ALIAS_BYTES: usize = 128;
pub const MAX_COMMAND_BYTES: usize = 256 * 1024;
pub const MAX_SECRET_BYTES: usize = 64 * 1024;
pub const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;

const TAG_PING: u8 = 1;
const TAG_STATUS: u8 = 2;
const TAG_DISCONNECT: u8 = 3;
const TAG_EXEC: u8 = 4;
const TAG_SHELL: u8 = 5;
const TAG_INPUT: u8 = 6;
const TAG_RESIZE: u8 = 7;
const TAG_EOF: u8 = 8;
const TAG_SHUTDOWN: u8 = 9;
const TAG_JUMP_AUTH_RESPONSE: u8 = 10;

const TAG_OK: u8 = 64;
const TAG_ERROR: u8 = 65;
const TAG_AUTH_REQUIRED: u8 = 66;
const TAG_STDOUT: u8 = 67;
const TAG_STDERR: u8 = 68;
const TAG_EXIT: u8 = 69;
const TAG_STATUS_RESPONSE: u8 = 70;
const TAG_CACHE: u8 = 71;
const TAG_JUMP_AUTH_CHALLENGE: u8 = 72;

#[derive(Clone)]
pub enum AuthRequest {
    Auto,
    Agent,
    Password(String),
    KeyboardInteractive(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
}

impl std::fmt::Debug for AuthRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => formatter.write_str("Auto"),
            Self::Agent => formatter.write_str("Agent"),
            Self::Password(_) => formatter.write_str("Password(<redacted>)"),
            Self::KeyboardInteractive(_) => {
                formatter.write_str("KeyboardInteractive(<redacted>)")
            }
            Self::PrivateKey { path, passphrase } => formatter
                .debug_struct("PrivateKey")
                .field("path", path)
                .field("has_passphrase", &passphrase.is_some())
                .finish(),
        }
    }
}

impl AuthRequest {
    pub fn into_authentication(self, configured_identities: Vec<PathBuf>) -> Authentication {
        match self {
            Self::Auto => Authentication::Auto {
                identity_files: configured_identities,
                passphrase: None,
            },
            Self::Agent => Authentication::Agent,
            Self::Password(secret) => Authentication::Password(secret),
            Self::KeyboardInteractive(secret) => Authentication::KeyboardInteractive(secret),
            Self::PrivateKey { path, passphrase } => {
                Authentication::PrivateKey { path, passphrase }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ClientFrame {
    Ping,
    Status,
    Disconnect { alias: String },
    Exec {
        alias: String,
        auth: Option<AuthRequest>,
        command: String,
        as_user: Option<String>,
    },
    Shell {
        alias: String,
        auth: Option<AuthRequest>,
        term: String,
        columns: u32,
        rows: u32,
        as_user: Option<String>,
    },
    Input(Vec<u8>),
    Resize { columns: u32, rows: u32 },
    Eof,
    Shutdown,
    JumpAuthResponse { auth: Option<AuthRequest> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerFrame {
    Ok,
    Error(String),
    AuthRequired,
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit(Option<u32>),
    Status {
        total_connections: usize,
        in_use_connections: usize,
        active_leases: usize,
        max_connections: usize,
        available_capacity: usize,
    },
    Cache { reused: bool },
    JumpAuthChallenge {
        index: usize,
        total: usize,
        attempt: usize,
        alias: String,
        host: String,
        port: u16,
        previous_failed: bool,
    },
}

pub async fn read_client_frame<R>(reader: &mut R) -> Result<Option<ClientFrame>>
where
    R: AsyncRead + Unpin,
{
    let Some(payload) = read_payload(reader).await? else {
        return Ok(None);
    };
    decode_client(&payload).map(Some)
}

pub async fn write_client_frame<W>(writer: &mut W, frame: &ClientFrame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let payload = encode_client(frame)?;
    write_payload(writer, &payload).await
}

pub async fn read_server_frame<R>(reader: &mut R) -> Result<Option<ServerFrame>>
where
    R: AsyncRead + Unpin,
{
    let Some(payload) = read_payload(reader).await? else {
        return Ok(None);
    };
    decode_server(&payload).map(Some)
}

pub async fn write_server_frame<W>(writer: &mut W, frame: &ServerFrame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let payload = encode_server(frame)?;
    write_payload(writer, &payload).await
}

async fn read_payload<R>(reader: &mut R) -> Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let mut first = [0_u8; 1];
    match reader.read_exact(&mut first).await {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error).context("failed to read daemon IPC frame length"),
    }
    let mut rest = [0_u8; 3];
    reader
        .read_exact(&mut rest)
        .await
        .context("daemon IPC frame length was truncated")?;
    let length = u32::from_be_bytes([first[0], rest[0], rest[1], rest[2]]) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        bail!("daemon IPC frame length {length} is outside 1..={MAX_FRAME_BYTES}");
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .context("failed to read daemon IPC frame payload")?;
    Ok(Some(payload))
}

async fn write_payload<W>(writer: &mut W, payload: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        bail!(
            "daemon IPC payload size {} is outside 1..={MAX_FRAME_BYTES}",
            payload.len()
        );
    }
    let length = u32::try_from(payload.len()).context("daemon IPC frame is too large")?;
    writer
        .write_all(&length.to_be_bytes())
        .await
        .context("failed to write daemon IPC frame length")?;
    writer
        .write_all(payload)
        .await
        .context("failed to write daemon IPC frame payload")?;
    writer.flush().await.context("failed to flush daemon IPC frame")
}

fn encode_client(frame: &ClientFrame) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match frame {
        ClientFrame::Ping => out.push(TAG_PING),
        ClientFrame::Status => out.push(TAG_STATUS),
        ClientFrame::Disconnect { alias } => {
            out.push(TAG_DISCONNECT);
            put_string(&mut out, alias, MAX_ALIAS_BYTES)?;
        }
        ClientFrame::Exec {
            alias,
            auth,
            command,
            as_user,
        } => {
            out.push(TAG_EXEC);
            put_string(&mut out, alias, MAX_ALIAS_BYTES)?;
            put_auth(&mut out, auth)?;
            put_string(&mut out, command, MAX_COMMAND_BYTES)?;
            put_optional_string(&mut out, as_user.as_deref(), MAX_ALIAS_BYTES)?;
        }
        ClientFrame::Shell {
            alias,
            auth,
            term,
            columns,
            rows,
            as_user,
        } => {
            out.push(TAG_SHELL);
            put_string(&mut out, alias, MAX_ALIAS_BYTES)?;
            put_auth(&mut out, auth)?;
            put_string(&mut out, term, 256)?;
            put_u32(&mut out, *columns);
            put_u32(&mut out, *rows);
            put_optional_string(&mut out, as_user.as_deref(), MAX_ALIAS_BYTES)?;
        }
        ClientFrame::Input(bytes) => {
            out.push(TAG_INPUT);
            put_bytes(&mut out, bytes, OUTPUT_CHUNK_BYTES)?;
        }
        ClientFrame::Resize { columns, rows } => {
            out.push(TAG_RESIZE);
            put_u32(&mut out, *columns);
            put_u32(&mut out, *rows);
        }
        ClientFrame::Eof => out.push(TAG_EOF),
        ClientFrame::Shutdown => out.push(TAG_SHUTDOWN),
        ClientFrame::JumpAuthResponse { auth } => {
            out.push(TAG_JUMP_AUTH_RESPONSE);
            put_auth(&mut out, auth)?;
        }
    }
    Ok(out)
}

fn decode_client(payload: &[u8]) -> Result<ClientFrame> {
    let mut cursor = Cursor::new(payload);
    let tag = cursor.u8()?;
    let frame = match tag {
        TAG_PING => ClientFrame::Ping,
        TAG_STATUS => ClientFrame::Status,
        TAG_DISCONNECT => ClientFrame::Disconnect {
            alias: cursor.string(MAX_ALIAS_BYTES)?,
        },
        TAG_EXEC => ClientFrame::Exec {
            alias: cursor.string(MAX_ALIAS_BYTES)?,
            auth: cursor.auth()?,
            command: cursor.string(MAX_COMMAND_BYTES)?,
            as_user: cursor.optional_string(MAX_ALIAS_BYTES)?,
        },
        TAG_SHELL => ClientFrame::Shell {
            alias: cursor.string(MAX_ALIAS_BYTES)?,
            auth: cursor.auth()?,
            term: cursor.string(256)?,
            columns: cursor.u32()?,
            rows: cursor.u32()?,
            as_user: cursor.optional_string(MAX_ALIAS_BYTES)?,
        },
        TAG_INPUT => ClientFrame::Input(cursor.bytes(OUTPUT_CHUNK_BYTES)?),
        TAG_RESIZE => ClientFrame::Resize {
            columns: cursor.u32()?,
            rows: cursor.u32()?,
        },
        TAG_EOF => ClientFrame::Eof,
        TAG_SHUTDOWN => ClientFrame::Shutdown,
        TAG_JUMP_AUTH_RESPONSE => ClientFrame::JumpAuthResponse {
            auth: cursor.auth()?,
        },
        _ => bail!("unknown daemon client frame tag {tag}"),
    };
    cursor.finish()?;
    Ok(frame)
}

fn encode_server(frame: &ServerFrame) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match frame {
        ServerFrame::Ok => out.push(TAG_OK),
        ServerFrame::Error(message) => {
            out.push(TAG_ERROR);
            put_string(&mut out, message, MAX_COMMAND_BYTES)?;
        }
        ServerFrame::AuthRequired => out.push(TAG_AUTH_REQUIRED),
        ServerFrame::Stdout(bytes) => {
            out.push(TAG_STDOUT);
            put_bytes(&mut out, bytes, OUTPUT_CHUNK_BYTES)?;
        }
        ServerFrame::Stderr(bytes) => {
            out.push(TAG_STDERR);
            put_bytes(&mut out, bytes, OUTPUT_CHUNK_BYTES)?;
        }
        ServerFrame::Exit(status) => {
            out.push(TAG_EXIT);
            match status {
                Some(status) => {
                    out.push(1);
                    put_u32(&mut out, *status);
                }
                None => out.push(0),
            }
        }
        ServerFrame::Status {
            total_connections,
            in_use_connections,
            active_leases,
            max_connections,
            available_capacity,
        } => {
            out.push(TAG_STATUS_RESPONSE);
            for value in [
                total_connections,
                in_use_connections,
                active_leases,
                max_connections,
                available_capacity,
            ] {
                put_u64(
                    &mut out,
                    u64::try_from(*value).context("daemon status counter exceeds u64")?,
                );
            }
        }
        ServerFrame::Cache { reused } => {
            out.push(TAG_CACHE);
            out.push(u8::from(*reused));
        }
        ServerFrame::JumpAuthChallenge {
            index,
            total,
            attempt,
            alias,
            host,
            port,
            previous_failed,
        } => {
            out.push(TAG_JUMP_AUTH_CHALLENGE);
            put_usize_u32(&mut out, *index, "jump index")?;
            put_usize_u32(&mut out, *total, "jump count")?;
            put_usize_u32(&mut out, *attempt, "jump auth attempt")?;
            put_string(&mut out, alias, MAX_ALIAS_BYTES)?;
            put_string(&mut out, host, 4096)?;
            put_u16(&mut out, *port);
            out.push(u8::from(*previous_failed));
        }
    }
    Ok(out)
}

fn decode_server(payload: &[u8]) -> Result<ServerFrame> {
    let mut cursor = Cursor::new(payload);
    let tag = cursor.u8()?;
    let frame = match tag {
        TAG_OK => ServerFrame::Ok,
        TAG_ERROR => ServerFrame::Error(cursor.string(MAX_COMMAND_BYTES)?),
        TAG_AUTH_REQUIRED => ServerFrame::AuthRequired,
        TAG_STDOUT => ServerFrame::Stdout(cursor.bytes(OUTPUT_CHUNK_BYTES)?),
        TAG_STDERR => ServerFrame::Stderr(cursor.bytes(OUTPUT_CHUNK_BYTES)?),
        TAG_EXIT => {
            let present = cursor.u8()?;
            ServerFrame::Exit(match present {
                0 => None,
                1 => Some(cursor.u32()?),
                _ => bail!("invalid daemon exit-status presence flag {present}"),
            })
        }
        TAG_STATUS_RESPONSE => ServerFrame::Status {
            total_connections: usize_from_u64(cursor.u64()?)?,
            in_use_connections: usize_from_u64(cursor.u64()?)?,
            active_leases: usize_from_u64(cursor.u64()?)?,
            max_connections: usize_from_u64(cursor.u64()?)?,
            available_capacity: usize_from_u64(cursor.u64()?)?,
        },
        TAG_CACHE => ServerFrame::Cache {
            reused: cursor.bool()?,
        },
        TAG_JUMP_AUTH_CHALLENGE => ServerFrame::JumpAuthChallenge {
            index: usize_from_u32(cursor.u32()?),
            total: usize_from_u32(cursor.u32()?),
            attempt: usize_from_u32(cursor.u32()?),
            alias: cursor.string(MAX_ALIAS_BYTES)?,
            host: cursor.string(4096)?,
            port: cursor.u16()?,
            previous_failed: cursor.bool()?,
        },
        _ => bail!("unknown daemon server frame tag {tag}"),
    };
    cursor.finish()?;
    Ok(frame)
}

fn put_auth(out: &mut Vec<u8>, auth: &Option<AuthRequest>) -> Result<()> {
    match auth {
        None => out.push(0),
        Some(AuthRequest::Auto) => out.push(1),
        Some(AuthRequest::Agent) => out.push(2),
        Some(AuthRequest::Password(secret)) => {
            out.push(3);
            put_string(out, secret, MAX_SECRET_BYTES)?;
        }
        Some(AuthRequest::KeyboardInteractive(secret)) => {
            out.push(4);
            put_string(out, secret, MAX_SECRET_BYTES)?;
        }
        Some(AuthRequest::PrivateKey { path, passphrase }) => {
            out.push(5);
            let path = path
                .to_str()
                .context("daemon private-key path is not valid UTF-8")?;
            put_string(out, path, 4096)?;
            put_optional_string(out, passphrase.as_deref(), MAX_SECRET_BYTES)?;
        }
    }
    Ok(())
}

fn put_optional_string(out: &mut Vec<u8>, value: Option<&str>, max: usize) -> Result<()> {
    match value {
        None => out.push(0),
        Some(value) => {
            out.push(1);
            put_string(out, value, max)?;
        }
    }
    Ok(())
}

fn put_string(out: &mut Vec<u8>, value: &str, max: usize) -> Result<()> {
    put_bytes(out, value.as_bytes(), max)
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<()> {
    if bytes.len() > max {
        bail!("daemon IPC field exceeds {max} bytes");
    }
    let length = u32::try_from(bytes.len()).context("daemon IPC field exceeds u32")?;
    put_u32(out, length);
    out.extend_from_slice(bytes);
    Ok(())
}

fn put_usize_u32(out: &mut Vec<u8>, value: usize, label: &str) -> Result<()> {
    let value = u32::try_from(value).with_context(|| format!("daemon {label} exceeds u32"))?;
    put_u32(out, value);
    Ok(())
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn usize_from_u32(value: u32) -> usize {
    value as usize
}

fn usize_from_u64(value: u64) -> Result<usize> {
    usize::try_from(value).context("daemon IPC counter does not fit usize")
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn u8(&mut self) -> Result<u8> {
        let value = *self
            .bytes
            .get(self.offset)
            .context("daemon IPC frame ended unexpectedly")?;
        self.offset += 1;
        Ok(value)
    }

    fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => bail!("invalid daemon boolean value {value}"),
        }
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes(bytes.try_into().expect("length checked")))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes(bytes.try_into().expect("length checked")))
    }

    fn u64(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_be_bytes(bytes.try_into().expect("length checked")))
    }

    fn bytes(&mut self, max: usize) -> Result<Vec<u8>> {
        let length = self.u32()? as usize;
        if length > max {
            bail!("daemon IPC field length {length} exceeds {max}");
        }
        Ok(self.take(length)?.to_vec())
    }

    fn string(&mut self, max: usize) -> Result<String> {
        String::from_utf8(self.bytes(max)?).context("daemon IPC string is not valid UTF-8")
    }

    fn optional_string(&mut self, max: usize) -> Result<Option<String>> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.string(max).map(Some),
            value => bail!("invalid daemon optional-string flag {value}"),
        }
    }

    fn auth(&mut self) -> Result<Option<AuthRequest>> {
        Ok(match self.u8()? {
            0 => None,
            1 => Some(AuthRequest::Auto),
            2 => Some(AuthRequest::Agent),
            3 => Some(AuthRequest::Password(self.string(MAX_SECRET_BYTES)?)),
            4 => Some(AuthRequest::KeyboardInteractive(
                self.string(MAX_SECRET_BYTES)?,
            )),
            5 => Some(AuthRequest::PrivateKey {
                path: PathBuf::from(self.string(4096)?),
                passphrase: self.optional_string(MAX_SECRET_BYTES)?,
            }),
            value => bail!("unknown daemon authentication kind {value}"),
        })
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .context("daemon IPC field length overflow")?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .context("daemon IPC frame ended unexpectedly")?;
        self.offset = end;
        Ok(slice)
    }

    fn finish(&self) -> Result<()> {
        if self.offset != self.bytes.len() {
            bail!("daemon IPC frame contains trailing bytes");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_round_trip_preserves_secret_only_on_wire() {
        let frame = ClientFrame::Exec {
            alias: "prod".into(),
            auth: Some(AuthRequest::Password("secret".into())),
            command: "uname -a".into(),
            as_user: Some("root".into()),
        };
        let encoded = encode_client(&frame).unwrap();
        let decoded = decode_client(&encoded).unwrap();
        let ClientFrame::Exec {
            alias,
            auth: Some(AuthRequest::Password(secret)),
            command,
            as_user,
        } = decoded
        else {
            panic!("unexpected frame")
        };
        assert_eq!(alias, "prod");
        assert_eq!(secret, "secret");
        assert_eq!(command, "uname -a");
        assert_eq!(as_user.as_deref(), Some("root"));
    }

    #[test]
    fn debug_never_exposes_auth_secret() {
        let value = format!("{:?}", AuthRequest::Password("supersecret".into()));
        assert!(!value.contains("supersecret"));
        assert!(value.contains("redacted"));
    }

    #[test]
    fn oversized_input_is_rejected() {
        assert!(
            encode_client(&ClientFrame::Input(vec![0; OUTPUT_CHUNK_BYTES + 1])).is_err()
        );
    }

    #[test]
    fn jump_challenge_round_trip_preserves_location_without_secret() {
        let frame = ServerFrame::JumpAuthChallenge {
            index: 1,
            total: 3,
            attempt: 2,
            alias: "inner".into(),
            host: "10.0.0.8".into(),
            port: 2222,
            previous_failed: true,
        };
        let encoded = encode_server(&frame).unwrap();
        assert_eq!(decode_server(&encoded).unwrap(), frame);
    }

    #[test]
    fn jump_auth_response_round_trip_redacts_debug_secret() {
        let frame = ClientFrame::JumpAuthResponse {
            auth: Some(AuthRequest::Password("jump-secret".into())),
        };
        let encoded = encode_client(&frame).unwrap();
        let decoded = decode_client(&encoded).unwrap();
        let text = format!("{decoded:?}");
        assert!(!text.contains("jump-secret"));
        assert!(text.contains("redacted"));
    }
}
