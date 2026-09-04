use std::io::{self, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use anyhow::Result;
use kaduox_ssh_core::{RemoteUser, SshClient};
use tokio::io::AsyncWrite;
use tokio::task::JoinSet;

const BROADCAST_OUTPUT_LIMIT: usize = 256 * 1024;

pub(crate) struct BroadcastTarget {
    pub(crate) label: String,
    pub(crate) client: Arc<SshClient>,
}

struct BroadcastResult {
    label: String,
    endpoint: String,
    exit_status: Option<u32>,
    stdout: CappedBuffer,
    stderr: CappedBuffer,
    error: Option<String>,
}

impl BroadcastResult {
    fn succeeded(&self) -> bool {
        self.error.is_none() && self.exit_status == Some(0)
    }
}

pub(crate) async fn execute(targets: Vec<BroadcastTarget>, command: String) -> Result<(usize, usize)> {
    if targets.is_empty() {
        return Ok((0, 0));
    }

    let mut tasks = JoinSet::new();
    let total = targets.len();
    for target in targets {
        tasks.spawn(run_target(target, command.clone()));
    }

    let mut completed = 0_usize;
    let mut failed = 0_usize;
    while let Some(joined) = tasks.join_next().await {
        completed += 1;
        match joined {
            Ok(result) => {
                if !result.succeeded() {
                    failed += 1;
                }
                print_result(&result, completed, total)?;
            }
            Err(error) => {
                failed += 1;
                eprintln!("=== broadcast worker failure [{completed}/{total}] ===\n{error}");
            }
        }
    }

    Ok((total, failed))
}

async fn run_target(target: BroadcastTarget, command: String) -> BroadcastResult {
    let config = target.client.config();
    let endpoint = format_endpoint(&config.host, config.port);
    let mut stdout = CappedBuffer::new(BROADCAST_OUTPUT_LIMIT);
    let mut stderr = CappedBuffer::new(BROADCAST_OUTPUT_LIMIT);
    let exec_result = target
        .client
        .exec_stream(
            &command,
            &RemoteUser::Current,
            &mut stdout,
            &mut stderr,
        )
        .await;

    match exec_result {
        Ok(exit_status) => BroadcastResult {
            label: target.label,
            endpoint,
            exit_status,
            stdout,
            stderr,
            error: None,
        },
        Err(error) => BroadcastResult {
            label: target.label,
            endpoint,
            exit_status: None,
            stdout,
            stderr,
            error: Some(format!("remote command failed: {error:#}")),
        },
    }
}

fn print_result(result: &BroadcastResult, completed: usize, total: usize) -> Result<()> {
    let mut output = io::stdout().lock();
    writeln!(
        output,
        "=== broadcast [{completed}/{total}] target={} endpoint={} exit={} ===",
        terminal_safe(&result.label),
        terminal_safe(&result.endpoint),
        result
            .exit_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "none".to_owned())
    )?;

    if !result.stdout.bytes.is_empty() {
        writeln!(
            output,
            "--- stdout{} ---",
            if result.stdout.truncated { " (truncated)" } else { "" }
        )?;
        writeln!(
            output,
            "{}",
            terminal_safe_output(&String::from_utf8_lossy(&result.stdout.bytes))
        )?;
    }
    if !result.stderr.bytes.is_empty() {
        writeln!(
            output,
            "--- stderr{} ---",
            if result.stderr.truncated { " (truncated)" } else { "" }
        )?;
        writeln!(
            output,
            "{}",
            terminal_safe_output(&String::from_utf8_lossy(&result.stderr.bytes))
        )?;
    }
    if let Some(error) = &result.error {
        writeln!(output, "--- error ---\n{}", terminal_safe(error))?;
    }
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

fn format_endpoint(host: &str, port: u16) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn terminal_safe(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{1b}' => output.push_str("\\x1b"),
            ch if ch.is_control() => output.push_str(&format!("\\u{{{:x}}}", u32::from(ch))),
            ch => output.push(ch),
        }
    }
    output
}

fn terminal_safe_output(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => output.push('\n'),
            '\t' => output.push('\t'),
            '\r' => output.push_str("\\r"),
            '\u{1b}' => output.push_str("\\x1b"),
            ch if ch.is_control() => output.push_str(&format!("\\u{{{:x}}}", u32::from(ch))),
            ch => output.push(ch),
        }
    }
    output
}

struct CappedBuffer {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl CappedBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            truncated: false,
        }
    }
}

impl AsyncWrite for CappedBuffer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        let retain = remaining.min(buf.len());
        if retain != 0 {
            self.bytes.extend_from_slice(&buf[..retain]);
        }
        if retain < buf.len() {
            self.truncated = true;
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn broadcast_output_is_bounded_while_fully_consumed() {
        let mut buffer = CappedBuffer::new(4);
        buffer.write_all(b"abcdefgh").await.unwrap();
        assert_eq!(buffer.bytes, b"abcd");
        assert!(buffer.truncated);
    }

    #[test]
    fn broadcast_output_escapes_terminal_controls() {
        assert_eq!(
            terminal_safe_output("line\n\u{1b}[31mred\r"),
            "line\n\\x1b[31mred\\r"
        );
    }
}
