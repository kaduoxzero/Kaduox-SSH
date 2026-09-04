use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};

use crate::transfer::{
    TransferCancellation, TransferDirection, TransferEvent, TransferSummary,
};

const DEFAULT_TRANSFER_TASK_RETENTION: usize = 128;
const MAX_TRANSFER_TASK_RETENTION: usize = 1024;
const MAX_TRANSFER_TASK_TEXT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferTaskId(u64);

impl TransferTaskId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferTaskKind {
    UploadFile,
    UploadDirectory,
    DownloadFile,
    DownloadDirectory,
}

impl TransferTaskKind {
    pub fn direction(self) -> TransferDirection {
        match self {
            Self::UploadFile | Self::UploadDirectory => TransferDirection::Upload,
            Self::DownloadFile | Self::DownloadDirectory => TransferDirection::Download,
        }
    }

    pub fn recursive(self) -> bool {
        matches!(self, Self::UploadDirectory | Self::DownloadDirectory)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferTaskState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl TransferTaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferTaskProgress {
    pub path: String,
    pub bytes_transferred: u64,
    pub total_bytes: Option<u64>,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferTaskSnapshot {
    pub id: TransferTaskId,
    pub kind: TransferTaskKind,
    pub source: String,
    pub destination: String,
    pub state: TransferTaskState,
    pub cancel_requested: bool,
    pub progress: Option<TransferTaskProgress>,
    pub summary: Option<TransferSummary>,
    pub error: Option<String>,
    pub retry_of: Option<TransferTaskId>,
}

#[derive(Debug, Clone)]
pub struct TransferTaskRegistration {
    id: TransferTaskId,
    cancellation: TransferCancellation,
}

impl TransferTaskRegistration {
    pub fn id(&self) -> TransferTaskId {
        self.id
    }

    pub fn cancellation(&self) -> TransferCancellation {
        self.cancellation.clone()
    }
}

#[derive(Clone)]
pub struct TransferTaskRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

struct RegistryInner {
    next_id: u64,
    max_retained: usize,
    tasks: HashMap<TransferTaskId, TaskRecord>,
    order: VecDeque<TransferTaskId>,
}

struct TaskRecord {
    snapshot: TransferTaskSnapshot,
    cancellation: TransferCancellation,
}

impl Default for TransferTaskRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                next_id: 1,
                max_retained: DEFAULT_TRANSFER_TASK_RETENTION,
                tasks: HashMap::new(),
                order: VecDeque::new(),
            })),
        }
    }
}

impl TransferTaskRegistry {
    pub fn new(max_retained: usize) -> Result<Self> {
        validate_retention(max_retained)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                next_id: 1,
                max_retained,
                tasks: HashMap::new(),
                order: VecDeque::new(),
            })),
        })
    }

    pub fn max_retained(&self) -> Result<usize> {
        Ok(self.lock()?.max_retained)
    }

    pub fn register(
        &self,
        kind: TransferTaskKind,
        source: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<TransferTaskRegistration> {
        let source = source.into();
        let destination = destination.into();
        validate_task_text("source", &source)?;
        validate_task_text("destination", &destination)?;
        let mut inner = self.lock()?;
        register_locked(&mut inner, kind, source, destination, None)
    }

    pub fn register_retry(&self, previous: TransferTaskId) -> Result<TransferTaskRegistration> {
        let mut inner = self.lock()?;
        let previous_snapshot = inner
            .tasks
            .get(&previous)
            .with_context(|| format!("unknown transfer task id {}", previous.get()))?
            .snapshot
            .clone();
        if !matches!(
            previous_snapshot.state,
            TransferTaskState::Cancelled | TransferTaskState::Failed
        ) {
            bail!(
                "transfer task {} can only be retried after cancellation or failure",
                previous.get()
            );
        }
        register_locked(
            &mut inner,
            previous_snapshot.kind,
            previous_snapshot.source,
            previous_snapshot.destination,
            Some(previous),
        )
    }

    pub fn mark_running(&self, id: TransferTaskId) -> Result<()> {
        let mut inner = self.lock()?;
        let record = task_mut(&mut inner, id)?;
        if record.snapshot.state != TransferTaskState::Queued {
            bail!(
                "transfer task {} cannot enter running state from {:?}",
                id.get(),
                record.snapshot.state
            );
        }
        record.snapshot.state = TransferTaskState::Running;
        Ok(())
    }

    pub fn record_progress(&self, id: TransferTaskId, event: &TransferEvent) -> Result<()> {
        let mut inner = self.lock()?;
        let record = task_mut(&mut inner, id)?;
        if record.snapshot.state != TransferTaskState::Running {
            bail!(
                "transfer task {} cannot record progress while {:?}",
                id.get(),
                record.snapshot.state
            );
        }
        if event.direction != record.snapshot.kind.direction() {
            bail!(
                "transfer task {} progress direction does not match task kind",
                id.get()
            );
        }
        record.snapshot.progress = Some(TransferTaskProgress {
            path: bounded_text(&event.path),
            bytes_transferred: event.bytes_transferred,
            total_bytes: event.total_bytes,
            completed: event.completed,
        });
        Ok(())
    }

    pub fn request_cancel(&self, id: TransferTaskId) -> Result<bool> {
        let mut inner = self.lock()?;
        let record = task_mut(&mut inner, id)?;
        if record.snapshot.state.is_terminal() {
            return Ok(false);
        }
        if record.snapshot.cancel_requested {
            return Ok(false);
        }
        record.snapshot.cancel_requested = true;
        record.cancellation.cancel();
        Ok(true)
    }

    pub fn complete(&self, id: TransferTaskId, summary: TransferSummary) -> Result<()> {
        let mut inner = self.lock()?;
        let record = task_mut(&mut inner, id)?;
        ensure_running(id, record.snapshot.state)?;
        record.snapshot.state = TransferTaskState::Completed;
        record.snapshot.summary = Some(summary);
        record.snapshot.error = None;
        Ok(())
    }

    pub fn mark_cancelled(&self, id: TransferTaskId) -> Result<()> {
        let mut inner = self.lock()?;
        let record = task_mut(&mut inner, id)?;
        if !matches!(
            record.snapshot.state,
            TransferTaskState::Queued | TransferTaskState::Running
        ) {
            bail!(
                "transfer task {} cannot be cancelled from {:?}",
                id.get(),
                record.snapshot.state
            );
        }
        record.snapshot.cancel_requested = true;
        record.cancellation.cancel();
        record.snapshot.state = TransferTaskState::Cancelled;
        Ok(())
    }

    pub fn fail(&self, id: TransferTaskId, error: impl Into<String>) -> Result<()> {
        let mut inner = self.lock()?;
        let record = task_mut(&mut inner, id)?;
        ensure_running(id, record.snapshot.state)?;
        record.snapshot.state = TransferTaskState::Failed;
        record.snapshot.error = Some(bounded_text(&error.into()));
        Ok(())
    }

    pub fn cancellation(&self, id: TransferTaskId) -> Result<TransferCancellation> {
        let inner = self.lock()?;
        Ok(inner
            .tasks
            .get(&id)
            .with_context(|| format!("unknown transfer task id {}", id.get()))?
            .cancellation
            .clone())
    }

    pub fn get(&self, id: TransferTaskId) -> Result<Option<TransferTaskSnapshot>> {
        let inner = self.lock()?;
        Ok(inner.tasks.get(&id).map(|record| record.snapshot.clone()))
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<TransferTaskSnapshot>> {
        let inner = self.lock()?;
        Ok(inner
            .order
            .iter()
            .rev()
            .take(limit)
            .filter_map(|id| inner.tasks.get(id).map(|record| record.snapshot.clone()))
            .collect())
    }

    pub fn clear_finished(&self) -> Result<usize> {
        let mut inner = self.lock()?;
        let finished = inner
            .order
            .iter()
            .copied()
            .filter(|id| {
                inner
                    .tasks
                    .get(id)
                    .is_some_and(|record| record.snapshot.state.is_terminal())
            })
            .collect::<Vec<_>>();
        for id in &finished {
            inner.tasks.remove(id);
        }
        inner.order.retain(|id| !finished.contains(id));
        Ok(finished.len())
    }

    fn lock(&self) -> Result<MutexGuard<'_, RegistryInner>> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("transfer task registry lock poisoned"))
    }
}

fn register_locked(
    inner: &mut RegistryInner,
    kind: TransferTaskKind,
    source: String,
    destination: String,
    retry_of: Option<TransferTaskId>,
) -> Result<TransferTaskRegistration> {
    make_capacity(inner)?;
    let id = TransferTaskId(inner.next_id);
    inner.next_id = inner
        .next_id
        .checked_add(1)
        .context("transfer task id counter overflow")?;
    let cancellation = TransferCancellation::default();
    let snapshot = TransferTaskSnapshot {
        id,
        kind,
        source,
        destination,
        state: TransferTaskState::Queued,
        cancel_requested: false,
        progress: None,
        summary: None,
        error: None,
        retry_of,
    };
    inner.tasks.insert(
        id,
        TaskRecord {
            snapshot,
            cancellation: cancellation.clone(),
        },
    );
    inner.order.push_back(id);
    Ok(TransferTaskRegistration { id, cancellation })
}

fn make_capacity(inner: &mut RegistryInner) -> Result<()> {
    while inner.tasks.len() >= inner.max_retained {
        let Some(position) = inner.order.iter().position(|id| {
            inner
                .tasks
                .get(id)
                .is_none_or(|record| record.snapshot.state.is_terminal())
        }) else {
            bail!(
                "transfer task registry is full with {} non-terminal tasks",
                inner.tasks.len()
            );
        };
        let id = inner
            .order
            .remove(position)
            .context("transfer task retention order changed unexpectedly")?;
        inner.tasks.remove(&id);
    }
    Ok(())
}

fn task_mut(inner: &mut RegistryInner, id: TransferTaskId) -> Result<&mut TaskRecord> {
    inner
        .tasks
        .get_mut(&id)
        .with_context(|| format!("unknown transfer task id {}", id.get()))
}

fn ensure_running(id: TransferTaskId, state: TransferTaskState) -> Result<()> {
    if state != TransferTaskState::Running {
        bail!(
            "transfer task {} cannot finish from {:?}",
            id.get(),
            state
        );
    }
    Ok(())
}

fn validate_retention(max_retained: usize) -> Result<()> {
    if max_retained == 0 {
        bail!("transfer task retention must be greater than zero");
    }
    if max_retained > MAX_TRANSFER_TASK_RETENTION {
        bail!(
            "transfer task retention must be <= {MAX_TRANSFER_TASK_RETENTION}"
        );
    }
    Ok(())
}

fn validate_task_text(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("transfer task {label} must not be empty");
    }
    if value.len() > MAX_TRANSFER_TASK_TEXT_BYTES {
        bail!(
            "transfer task {label} exceeds {MAX_TRANSFER_TASK_TEXT_BYTES} UTF-8 bytes"
        );
    }
    Ok(())
}

fn bounded_text(value: &str) -> String {
    if value.len() <= MAX_TRANSFER_TASK_TEXT_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_TRANSFER_TASK_TEXT_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut output = value[..end].to_owned();
    output.push_str("...");
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_retention_is_bounded() {
        assert!(TransferTaskRegistry::new(0).is_err());
        assert!(TransferTaskRegistry::new(MAX_TRANSFER_TASK_RETENTION + 1).is_err());
        assert!(TransferTaskRegistry::new(1).is_ok());
    }

    #[test]
    fn lifecycle_records_progress_and_cancellation() {
        let registry = TransferTaskRegistry::new(4).unwrap();
        let task = registry
            .register(TransferTaskKind::DownloadFile, "/remote/a", "local-a")
            .unwrap();
        registry.mark_running(task.id()).unwrap();
        registry
            .record_progress(
                task.id(),
                &TransferEvent {
                    direction: TransferDirection::Download,
                    path: "/remote/a".to_owned(),
                    bytes_transferred: 32,
                    total_bytes: Some(64),
                    completed: false,
                },
            )
            .unwrap();
        assert!(registry.request_cancel(task.id()).unwrap());
        assert!(task.cancellation().is_cancelled());
        registry.mark_cancelled(task.id()).unwrap();

        let snapshot = registry.get(task.id()).unwrap().unwrap();
        assert_eq!(snapshot.state, TransferTaskState::Cancelled);
        assert!(snapshot.cancel_requested);
        assert_eq!(snapshot.progress.unwrap().bytes_transferred, 32);
    }

    #[test]
    fn retry_keeps_paths_and_lineage() {
        let registry = TransferTaskRegistry::new(4).unwrap();
        let first = registry
            .register(TransferTaskKind::UploadDirectory, "local", "/remote")
            .unwrap();
        registry.mark_running(first.id()).unwrap();
        registry.fail(first.id(), "network failed").unwrap();

        let retry = registry.register_retry(first.id()).unwrap();
        let snapshot = registry.get(retry.id()).unwrap().unwrap();
        assert_eq!(snapshot.source, "local");
        assert_eq!(snapshot.destination, "/remote");
        assert_eq!(snapshot.retry_of, Some(first.id()));
        assert_eq!(snapshot.state, TransferTaskState::Queued);
    }

    #[test]
    fn completed_entries_are_pruned_before_active_tasks() {
        let registry = TransferTaskRegistry::new(2).unwrap();
        let first = registry
            .register(TransferTaskKind::UploadFile, "a", "b")
            .unwrap();
        registry.mark_running(first.id()).unwrap();
        registry.complete(first.id(), TransferSummary::default()).unwrap();
        let second = registry
            .register(TransferTaskKind::UploadFile, "c", "d")
            .unwrap();
        let third = registry
            .register(TransferTaskKind::UploadFile, "e", "f")
            .unwrap();

        assert!(registry.get(first.id()).unwrap().is_none());
        assert!(registry.get(second.id()).unwrap().is_some());
        assert!(registry.get(third.id()).unwrap().is_some());
        assert!(registry
            .register(TransferTaskKind::UploadFile, "g", "h")
            .is_err());
    }

    #[test]
    fn wrong_progress_direction_fails_closed() {
        let registry = TransferTaskRegistry::new(2).unwrap();
        let task = registry
            .register(TransferTaskKind::UploadFile, "local", "/remote")
            .unwrap();
        registry.mark_running(task.id()).unwrap();
        assert!(registry
            .record_progress(
                task.id(),
                &TransferEvent {
                    direction: TransferDirection::Download,
                    path: "/remote".to_owned(),
                    bytes_transferred: 1,
                    total_bytes: None,
                    completed: false,
                },
            )
            .is_err());
    }
}
