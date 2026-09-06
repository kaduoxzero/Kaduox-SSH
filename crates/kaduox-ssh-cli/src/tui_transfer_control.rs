use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kaduox_ssh_core::TransferCancellation;

pub(crate) struct TransferCancelListener {
    stop: Arc<AtomicBool>,
    cancellation: TransferCancellation,
    handle: tokio::task::JoinHandle<Result<()>>,
}

impl TransferCancelListener {
    pub(crate) fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let cancellation = TransferCancellation::default();
        let thread_stop = Arc::clone(&stop);
        let thread_cancellation = cancellation.clone();
        let handle = tokio::task::spawn_blocking(move || -> Result<()> {
            while !thread_stop.load(Ordering::Acquire) {
                if !event::poll(Duration::from_millis(100))
                    .context("failed to poll terminal input during transfer")?
                {
                    continue;
                }
                let input =
                    event::read().context("failed to read terminal input during transfer")?;
                let Event::Key(key) = input else {
                    continue;
                };
                if is_transfer_cancel_key(key) {
                    thread_cancellation.cancel();
                }
            }
            Ok(())
        });
        Self {
            stop,
            cancellation,
            handle,
        }
    }

    pub(crate) fn cancellation(&self) -> TransferCancellation {
        self.cancellation.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        self.handle
            .await
            .context("transfer cancellation listener task failed")??;
        Ok(())
    }
}

fn is_transfer_cancel_key(key: KeyEvent) -> bool {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return false;
    }
    key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_cancel_keys_are_explicit() {
        assert!(is_transfer_cancel_key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )));
        assert!(is_transfer_cancel_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_transfer_cancel_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
        assert!(!is_transfer_cancel_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));
    }
}
