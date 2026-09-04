use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::client::SshClient;
use crate::transfer::{TransferEvent, TransferOptions, TransferSummary};
use crate::transfer_task::{
    TransferTaskId, TransferTaskKind, TransferTaskRegistration, TransferTaskRegistry,
};

const TRACKED_PROGRESS_QUEUE: usize = 64;
const CALLER_CANCELLATION_POLL: Duration = Duration::from_millis(50);

pub struct TransferTaskManager<'a> {
    client: &'a SshClient,
    registry: TransferTaskRegistry,
}

impl<'a> TransferTaskManager<'a> {
    pub fn new(client: &'a SshClient, registry: TransferTaskRegistry) -> Self {
        Self { client, registry }
    }

    pub fn registry(&self) -> TransferTaskRegistry {
        self.registry.clone()
    }

    pub async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
    ) -> Result<u64> {
        let source = local_task_path(local_path)?;
        let summary = self
            .run_new(
                TransferTaskKind::UploadFile,
                source,
                remote_path.to_owned(),
                options,
            )
            .await?;
        Ok(summary.bytes)
    }

    pub async fn upload_directory(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        let source = local_task_path(local_path)?;
        self.run_new(
            TransferTaskKind::UploadDirectory,
            source,
            remote_path.to_owned(),
            options,
        )
        .await
    }

    pub async fn download_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        options: TransferOptions,
    ) -> Result<u64> {
        let destination = local_task_path(local_path)?;
        let summary = self
            .run_new(
                TransferTaskKind::DownloadFile,
                remote_path.to_owned(),
                destination,
                options,
            )
            .await?;
        Ok(summary.bytes)
    }

    pub async fn download_directory(
        &self,
        remote_path: &str,
        local_path: &Path,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        let destination = local_task_path(local_path)?;
        self.run_new(
            TransferTaskKind::DownloadDirectory,
            remote_path.to_owned(),
            destination,
            options,
        )
        .await
    }

    /// Retry a failed or cancelled task using the same source/destination pair.
    ///
    /// Retry always enables the canonical transfer `resume` policy. The new task
    /// receives a fresh task id and retains `retry_of` lineage in the registry.
    pub async fn retry(
        &self,
        previous: TransferTaskId,
        mut options: TransferOptions,
    ) -> Result<TransferSummary> {
        let snapshot = self
            .registry
            .get(previous)?
            .with_context(|| format!("unknown transfer task id {}", previous.get()))?;
        options.resume = true;
        let registration = self.registry.register_retry(previous)?;
        self.run_registered(
            registration,
            snapshot.kind,
            snapshot.source,
            snapshot.destination,
            options,
        )
        .await
    }

    async fn run_new(
        &self,
        kind: TransferTaskKind,
        source: String,
        destination: String,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        let registration = self
            .registry
            .register(kind, source.clone(), destination.clone())?;
        self.run_registered(registration, kind, source, destination, options)
            .await
    }

    async fn run_registered(
        &self,
        registration: TransferTaskRegistration,
        kind: TransferTaskKind,
        source: String,
        destination: String,
        mut options: TransferOptions,
    ) -> Result<TransferSummary> {
        let id = registration.id();
        self.registry.mark_running(id)?;

        let caller_cancellation = options.cancellation.clone();
        let task_cancellation = registration.cancellation();
        let downstream_progress = options.progress.take();
        let (progress_tx, progress_rx) = mpsc::channel(TRACKED_PROGRESS_QUEUE);
        options.cancellation = task_cancellation.clone();
        options.progress = Some(progress_tx);

        let bridge = spawn_tracking_bridge(
            self.registry.clone(),
            id,
            progress_rx,
            downstream_progress,
            caller_cancellation,
        );

        let result = match kind {
            TransferTaskKind::UploadFile => self
                .client
                .upload_with_options(Path::new(&source), &destination, options)
                .await
                .map(|bytes| TransferSummary {
                    files: 1,
                    bytes,
                    ..Default::default()
                }),
            TransferTaskKind::UploadDirectory => self
                .client
                .upload_recursive(Path::new(&source), &destination, options)
                .await,
            TransferTaskKind::DownloadFile => self
                .client
                .download_with_options(&source, Path::new(&destination), options)
                .await
                .map(|bytes| TransferSummary {
                    files: 1,
                    bytes,
                    ..Default::default()
                }),
            TransferTaskKind::DownloadDirectory => self
                .client
                .download_recursive(&source, Path::new(&destination), options)
                .await,
        };

        if let Err(error) = await_tracking_bridge(bridge).await {
            let _ = self
                .registry
                .fail(id, format!("transfer progress tracking failed: {error:#}"));
            return Err(error);
        }

        match result {
            Ok(summary) => {
                self.registry.complete(id, summary)?;
                Ok(summary)
            }
            Err(error) => {
                if task_cancellation.is_cancelled() {
                    self.registry.mark_cancelled(id)?;
                } else {
                    self.registry.fail(id, format!("{error:#}"))?;
                }
                Err(error)
            }
        }
    }
}

fn spawn_tracking_bridge(
    registry: TransferTaskRegistry,
    id: TransferTaskId,
    mut progress_rx: mpsc::Receiver<TransferEvent>,
    downstream_progress: Option<mpsc::Sender<TransferEvent>>,
    caller_cancellation: crate::transfer::TransferCancellation,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = progress_rx.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    registry.record_progress(id, &event)?;
                    if let Some(sender) = downstream_progress.as_ref() {
                        let _ = sender.try_send(event);
                    }
                }
                _ = sleep(CALLER_CANCELLATION_POLL) => {
                    if caller_cancellation.is_cancelled() {
                        let _ = registry.request_cancel(id)?;
                    }
                }
            }
        }
        Ok(())
    })
}

async fn await_tracking_bridge(handle: JoinHandle<Result<()>>) -> Result<()> {
    handle
        .await
        .context("tracked transfer progress task failed")??;
    Ok(())
}

fn local_task_path(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .with_context(|| {
            format!(
                "tracked transfer local path must be valid UTF-8 for reliable retry: {}",
                path.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_local_path_requires_utf8_when_platform_allows_non_utf8() {
        assert_eq!(local_task_path(Path::new("local/file")).unwrap(), "local/file");
    }

    #[test]
    fn tracked_progress_queue_is_bounded() {
        assert_eq!(TRACKED_PROGRESS_QUEUE, 64);
    }
}
