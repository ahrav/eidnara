//! Records a frozen Canonical composition as a Kernel use-outcome serve: which decision objects one session was served.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{CommitIntent, CommitReceipt, Envelope, KernelError, SessionRole, UseOutcomeSpec};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::canonical_memory::ServedSet;
use crate::kernel_routes::read::MAX_READ_ROWS;
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
/// Heap bytes a served id holds beside its text: its string header and the vector slot.
const SERVED_ID_OVERHEAD_BYTES: u64 = 32;
/// The ids' bytes: a job is handed off only after the pass's Memory Store commit, which stores the rendered block
/// holding every served id in full as one durable string bounded by `memory_store::MAX_DURABLE_TEXT_BYTES`; a block past
/// it fails the commit and no job is handed off. The read bounds the id count by [`MAX_READ_ROWS`].
const SERVED_IDS_BYTES_BOUND: u64 =
    memory_store::MAX_DURABLE_TEXT_BYTES as u64 + MAX_READ_ROWS as u64 * SERVED_ID_OVERHEAD_BYTES;
/// The identity strings a job carries beside its ids: the project root and digest, harness, session id, operation
/// key, request digest, and the digest strings of the set and the spec. The host bounds the bound route's root at
/// 4096 bytes, its harness at 128, and its session at 256 (`host_runtime::control`), and a transform runs only on the
/// route whose binding names its session, so the sum stays near 5 KiB.
const JOB_IDENTITY_BYTES: u64 = 16 * 1024;
/// One in-flight write at its peak: the owned [`ServedSet`], the `&str` slice `request_digest` reads, the owned id
/// copy the insert spec carries into the commit, and the identities.
const RETAINED_SERVED_SET_BYTES_PER_WRITE: u64 =
    2 * SERVED_IDS_BYTES_BOUND + MAX_READ_ROWS as u64 * 16 + JOB_IDENTITY_BYTES;
/// What in-flight jobs retain after their passes' responses released their charges: a slot is reserved before the set
/// is built, so at most [`WRITES_IN_FLIGHT`] jobs exist.
pub(crate) const RETAINED_SERVED_SET_BYTES_BOUND: u64 =
    WRITES_IN_FLIGHT as u64 * RETAINED_SERVED_SET_BYTES_PER_WRITE;

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

    /// Reserves a write slot before the caller clones what the job retains. Slot exhaustion counts `deadline` and
    /// returns `None`; a reservation dropped without a job releases the slot.
    pub(crate) fn reserve(&self) -> Option<OwnedSemaphorePermit> {
        match Arc::clone(&self.slots).try_acquire_owned() {
            Ok(slot) => Some(slot),
            Err(_) => {
                self.record(UseOutcomeWrite::Deadline);
                None
            }
        }
    }

    /// The returned job holds `slot` through `write` and the outcome count, or until it is dropped. `write` returning `None` counts `kernel_unavailable`.
    pub(crate) fn job(
        self: &Arc<Self>,
        slot: OwnedSemaphorePermit,
        write: impl FnOnce() -> Option<UseOutcomeWrite> + Send + 'static,
    ) -> impl FnOnce() + Send + 'static {
        let writes = Arc::clone(self);
        move || {
            writes.record(write().unwrap_or(UseOutcomeWrite::KernelUnavailable));
            drop(slot);
        }
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

    /// Write slots a job holds at this instant.
    #[cfg(test)]
    pub(crate) fn writes_in_flight(&self) -> usize {
        WRITES_IN_FLIGHT - self.slots.available_permits()
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
    pub(crate) served: &'a ServedSet,
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
    let served_ids: Vec<&str> = serve.served.ids().collect();
    let digest = serve.served.digest();
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
            composition_revision: serve.served.revision(),
            served_commit_seq: serve.served.known_as_of(),
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
