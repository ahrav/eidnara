//! Records a frozen Canonical composition as a Kernel use-outcome serve: which decision objects one session was served.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{CommitIntent, CommitReceipt, Envelope, KernelError, SessionRole, UseOutcomeSpec};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::canonical_memory::CanonicalMemorySnapshot;
use crate::kernel_routes::{KernelOpenCoordinator, ProjectBinding};

const PRODUCER: &str = "eidnara-daemon/use-outcome";
const FAMILY: &str = "use-outcome";
const REQUEST_DOMAIN: &[u8] = b"eidnara-use-outcome-serve-v1\n";
/// `WRITE_BUDGET` bounds writer waits and the pooled-reader replay check; the insert runs to completion once the writer is held.
pub(crate) const WRITE_BUDGET: Duration = Duration::from_millis(250);
const WRITER_FIRST_WAIT: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UseOutcomeWrite {
    Written,
    Replayed,
    Conflict,
    Deadline,
    KernelUnavailable,
    Refused,
}

impl UseOutcomeWrite {
    pub(crate) const ALL: [Self; 6] = [
        Self::Written,
        Self::Replayed,
        Self::Conflict,
        Self::Deadline,
        Self::KernelUnavailable,
        Self::Refused,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Replayed => "replayed",
            Self::Conflict => "conflict",
            Self::Deadline => "deadline",
            Self::KernelUnavailable => "kernel_unavailable",
            Self::Refused => "refused",
        }
    }

    pub(crate) fn from_error(error: KernelError) -> Self {
        match error {
            KernelError::Conflict => Self::Conflict,
            KernelError::Deadline | KernelError::Busy => Self::Deadline,
            KernelError::Held
            | KernelError::EngineUnsupported
            | KernelError::Foreign
            | KernelError::Inconclusive
            | KernelError::Io
            | KernelError::IdentityMismatch
            | KernelError::FenceLost
            | KernelError::InvalidRestore => Self::KernelUnavailable,
            KernelError::CorruptCanonicalRow
            | KernelError::InvalidInput
            | KernelError::AdmissionPolicy
            | KernelError::PreviewAuthorityChanged
            | KernelError::FutureSnapshot
            | KernelError::NotFound
            | KernelError::InvalidCheckpoint
            | KernelError::NoRequiredConsumers
            | KernelError::ConsumerPending
            | KernelError::Fault
            | KernelError::UnsafeDestination
            | KernelError::InvalidBackup => Self::Refused,
        }
    }
}

const _: () = assert!(UseOutcomeWrite::Refused as usize + 1 == UseOutcomeWrite::ALL.len());

const WRITES_IN_FLIGHT: usize = 4;
/// One retained composition: the figure `memory_capture::RETAINED_BYTES_BOUND` declares per worker for the same
/// bounded canonical read, 8 MiB of decision payload plus the rows' ids, categories, and string headers, which
/// the read's payload cut leaves uncounted.
const RETAINED_SNAPSHOT_BYTES_PER_WRITE: u64 = 32 * 1024 * 1024;
/// The compositions in-flight jobs retain after their passes' responses released their charges: at most
/// [`WRITES_IN_FLIGHT`] clones.
pub(crate) const RETAINED_SNAPSHOT_BYTES_BOUND: u64 =
    WRITES_IN_FLIGHT as u64 * RETAINED_SNAPSHOT_BYTES_PER_WRITE;

#[derive(Debug)]
pub(crate) struct UseOutcomeWrites {
    counts: [AtomicU64; UseOutcomeWrite::ALL.len()],
    slots: Arc<Semaphore>,
    /// The writer wait each job is given, in milliseconds; [`WRITE_BUDGET`] outside tests.
    write_budget_ms: AtomicU64,
}

impl Default for UseOutcomeWrites {
    fn default() -> Self {
        Self {
            counts: Default::default(),
            slots: Arc::new(Semaphore::new(WRITES_IN_FLIGHT)),
            write_budget_ms: AtomicU64::new(WRITE_BUDGET.as_millis() as u64),
        }
    }
}

impl UseOutcomeWrites {
    pub(crate) fn record(&self, outcome: UseOutcomeWrite) {
        self.counts[outcome as usize].fetch_add(1, Ordering::Relaxed);
        if !matches!(
            outcome,
            UseOutcomeWrite::Written | UseOutcomeWrite::Replayed
        ) {
            eprintln!("daemon: use-outcome serve write: {}", outcome.name());
        }
    }

    /// The returned job holds a write slot through `write` and the outcome count, or until it is dropped. `write` returning `None` counts `kernel_unavailable`. Slot exhaustion counts `deadline` and returns `None`.
    pub(crate) fn job(
        self: &Arc<Self>,
        write: impl FnOnce() -> Option<UseOutcomeWrite> + Send + 'static,
    ) -> Option<impl FnOnce() + Send + 'static> {
        let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() else {
            self.record(UseOutcomeWrite::Deadline);
            return None;
        };
        let writes = Arc::clone(self);
        Some(move || {
            writes.record(write().unwrap_or(UseOutcomeWrite::KernelUnavailable));
            drop(slot);
        })
    }

    pub(crate) fn write_budget(&self) -> Duration {
        Duration::from_millis(self.write_budget_ms.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn get(&self, outcome: UseOutcomeWrite) -> u64 {
        self.counts[outcome as usize].load(Ordering::Relaxed)
    }

    /// Lengthens or shortens the writer wait of every later job, so a test can hold the Kernel writer across a pass without racing the budget.
    #[cfg(test)]
    pub(crate) fn set_write_budget(&self, budget: Duration) {
        self.write_budget_ms
            .store(budget.as_millis() as u64, Ordering::Relaxed);
    }

    /// Waits until completed or dropped jobs have released every write slot.
    #[cfg(test)]
    pub(crate) async fn settled(&self) {
        let _every_slot = self
            .slots
            .acquire_many(WRITES_IN_FLIGHT as u32)
            .await
            .expect("the slots semaphore stays open");
    }

    #[cfg(test)]
    pub(crate) fn occupy_every_slot(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.slots
            .try_acquire_many(WRITES_IN_FLIGHT as u32)
            .expect("no write is in flight")
    }
}

pub(crate) struct Serve<'a> {
    pub(crate) project: &'a ProjectBinding,
    pub(crate) harness: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) is_subagent: bool,
    pub(crate) snapshot: &'a CanonicalMemorySnapshot,
    pub(crate) recorded_at: i64,
}

/// Writes the serve record, or answers `None` when the Kernel is not Ready and no write was attempted.
/// The first commit waits at most [`WRITER_FIRST_WAIT`] for the writer. When another writer holds it longer, a served set already
/// recorded under the key replays from a pooled reader, and a new one waits for the writer until `write_budget` ends
/// ([`WRITE_BUDGET`] in production).
/// A reader replay reads only the stored receipt, which commits with its record; the writer's fence check applies to commits.
/// `kernel_unavailable` from a commit can follow a writer wait, and after a post-commit `Io` or `Inconclusive`
/// the record may exist; a later write at the same key replays it.
pub(crate) fn record_serve(
    kernel: &KernelOpenCoordinator,
    serve: &Serve<'_>,
    write_budget: Duration,
) -> Option<UseOutcomeWrite> {
    let store = kernel.kernel_store().ok()?;
    let snapshot = serve.snapshot;
    let served_ids = snapshot.served_ids();
    let digest = snapshot.served_set_digest();
    let Some(operation_key) = serve
        .project
        .operation_key(FAMILY, &format!("{}:{digest}", serve.session_id))
    else {
        return Some(UseOutcomeWrite::Refused);
    };
    let session_role = if serve.is_subagent {
        SessionRole::Subagent
    } else {
        SessionRole::Primary
    };
    let request_digest = request_digest(&served_ids, serve.harness, session_role);
    let intent = || CommitIntent {
        producer: PRODUCER.to_string(),
        operation_key: operation_key.clone(),
        request_digest: request_digest.clone(),
        actor: "daemon".to_string(),
        cause: "use outcome served".to_string(),
    };
    let insert = |envelope: &mut Envelope<'_>| {
        envelope.insert_use_outcome(UseOutcomeSpec {
            project_digest: serve.project.digest().to_string(),
            harness: serve.harness.to_string(),
            session_id: serve.session_id.to_string(),
            served_set_digest: digest.to_string(),
            composition_revision: snapshot.revision(),
            served_commit_seq: snapshot.known_as_of(),
            session_role,
            recorded_at: serve.recorded_at,
            decision_object_ids: served_ids.iter().map(|id| (*id).to_string()).collect(),
        })
    };
    let commit = |until: Instant| -> Result<CommitReceipt, KernelError> {
        let budget = EvalBudget::new(Some(until), Arc::default());
        store.commit_within_budget(&budget, intent(), insert)
    };
    let deadline = Instant::now() + write_budget;
    let outcome = match commit(Instant::now() + WRITER_FIRST_WAIT) {
        Err(KernelError::Deadline) => store
            .preview(deadline, |preview| preview.stored_receipt(intent()))
            .and_then(|(_, stored)| stored.map_or_else(|| commit(deadline), Ok)),
        settled => settled,
    };
    Some(match outcome {
        Ok(receipt) if receipt.replayed => UseOutcomeWrite::Replayed,
        Ok(_) => UseOutcomeWrite::Written,
        Err(error) => UseOutcomeWrite::from_error(error),
    })
}

fn request_digest(served_ids: &[&str], harness: &str, role: SessionRole) -> String {
    let mut hash = Sha256::new();
    hash.update(REQUEST_DOMAIN);
    for field in served_ids.iter().copied().chain([harness, role.as_str()]) {
        hash.update(field.len().to_string().as_bytes());
        hash.update(b":");
        hash.update(field.as_bytes());
        hash.update(b"\n");
    }
    format!("{:x}", hash.finalize())
}
