//! Records a frozen Canonical composition as a Kernel use-outcome serve: which decision objects one session was served. Also writes the record lifecycle: `session.delete` marks a session's live records deleted, and a committing pass advances their `last_transform_at`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{
    CommitIntent, CommitReceipt, Envelope, KernelError, KernelStore, SessionRole, UseOutcomeSpec,
};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::canonical_memory::CanonicalMemorySnapshot;
use crate::kernel_routes::read::MAX_READ_ROWS;
use crate::kernel_routes::{KernelOpenCoordinator, ProjectBinding};

const PRODUCER: &str = "eidnara-daemon/use-outcome";
const FAMILY: &str = "use-outcome";
const REQUEST_DOMAIN: &[u8] = b"eidnara-use-outcome-serve-v1\n";
const DELETE_PRODUCER: &str = "eidnara-daemon/use-outcome-delete";
const DELETE_FAMILY: &str = "use-outcome-delete";
const DELETE_REQUEST_DOMAIN: &[u8] = b"eidnara-use-outcome-delete-v1\n";
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
/// Heap bytes a snapshot row holds beside its rendered text: three string headers and the vector slot.
const SNAPSHOT_ROW_OVERHEAD_BYTES: u64 = 128;
/// One retained composition. A job is handed off only after the pass's Memory Store commit, which stores the rendered
/// block, every served row's id, category, and content, as one durable string bounded by
/// `memory_store::MAX_DURABLE_TEXT_BYTES`; a block past it fails the commit and no job is handed off. The read bounds
/// the row count by [`MAX_READ_ROWS`].
const RETAINED_SNAPSHOT_BYTES_PER_WRITE: u64 = memory_store::MAX_DURABLE_TEXT_BYTES as u64
    + MAX_READ_ROWS as u64 * SNAPSHOT_ROW_OVERHEAD_BYTES;
/// The compositions in-flight jobs retain after their passes' responses released their charges: a slot is reserved
/// before the composition is cloned, so at most [`WRITES_IN_FLIGHT`] clones exist.
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

    #[cfg(test)]
    pub(crate) fn occupy_every_slot(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.slots
            .try_acquire_many(WRITES_IN_FLIGHT as u32)
            .expect("no write is in flight")
    }
}

/// Closed names for the lifecycle writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleWrite {
    DeletedMarked,
    DeleteNothingLive,
    DeleteKernelUnavailable,
    TransformTouched,
    TransformSkippedDeadline,
    TransformSkippedUnavailable,
}

impl LifecycleWrite {
    pub(crate) const ALL: [Self; 6] = [
        Self::DeletedMarked,
        Self::DeleteNothingLive,
        Self::DeleteKernelUnavailable,
        Self::TransformTouched,
        Self::TransformSkippedDeadline,
        Self::TransformSkippedUnavailable,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DeletedMarked => "deleted_marked",
            Self::DeleteNothingLive => "delete_nothing_live",
            Self::DeleteKernelUnavailable => "delete_kernel_unavailable",
            Self::TransformTouched => "transform_touched",
            Self::TransformSkippedDeadline => "transform_skipped_deadline",
            Self::TransformSkippedUnavailable => "transform_skipped_unavailable",
        }
    }

    const fn logged(self) -> bool {
        match self {
            Self::DeletedMarked | Self::DeleteNothingLive | Self::TransformTouched => false,
            Self::DeleteKernelUnavailable
            | Self::TransformSkippedDeadline
            | Self::TransformSkippedUnavailable => true,
        }
    }
}

const _: () =
    assert!(LifecycleWrite::TransformSkippedUnavailable as usize + 1 == LifecycleWrite::ALL.len());

#[derive(Debug, Default)]
pub(crate) struct LifecycleCounts([AtomicU64; LifecycleWrite::ALL.len()]);

impl LifecycleCounts {
    pub(crate) fn record(&self, write: LifecycleWrite) {
        self.0[write as usize].fetch_add(1, Ordering::Relaxed);
        if write.logged() {
            eprintln!("daemon: use-outcome lifecycle write: {}", write.name());
        }
    }

    #[cfg(test)]
    pub(crate) fn get(&self, write: LifecycleWrite) -> u64 {
        self.0[write as usize].load(Ordering::Relaxed)
    }
}

/// `session.delete`'s use-outcome step: one envelope that sets `deleted_at = now` on every live record of the session, selected under the writer. An envelope that marks nothing rolls back, so a session with no live record commits nothing. The key carries a digest of the session id, so the Kernel's identity check on the key passes for any id the wire admits, and a fresh attempt nonce, so a receipt never answers a later delete as a replay; idempotence rests on the live selection, and a record already marked keeps its first timestamp.
pub(crate) fn mark_session_deleted(
    store: &KernelStore,
    project: &ProjectBinding,
    harness: &str,
    session_id: &str,
    now: i64,
) -> Result<LifecycleWrite, KernelError> {
    let mut nonce = [0u8; 16];
    getrandom::getrandom(&mut nonce).map_err(|_| KernelError::Io)?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let session_digest = length_prefixed_digest(DELETE_REQUEST_DOMAIN, [session_id].into_iter());
    let operation_key = project
        .operation_key(DELETE_FAMILY, &format!("{session_digest}:{nonce}"))
        .ok_or(KernelError::InvalidInput)?;
    let now_text = now.to_string();
    let intent = CommitIntent {
        producer: DELETE_PRODUCER.to_string(),
        operation_key,
        request_digest: length_prefixed_digest(
            DELETE_REQUEST_DOMAIN,
            [session_id, now_text.as_str()].into_iter(),
        ),
        actor: "daemon".to_string(),
        cause: "session deleted".to_string(),
    };
    let mut nothing_live = false;
    let outcome = store.commit(intent, |envelope| {
        let marked =
            envelope.mark_use_outcomes_deleted(project.digest(), harness, session_id, now)?;
        nothing_live = marked == 0;
        if nothing_live {
            return Err(KernelError::NotFound);
        }
        Ok(marked.to_string())
    });
    match outcome {
        Ok(_) => Ok(LifecycleWrite::DeletedMarked),
        Err(KernelError::NotFound) if nothing_live => Ok(LifecycleWrite::DeleteNothingLive),
        Err(error) => Err(error),
    }
}

/// Advances `last_transform_at` to `at` on the session's live records after a committed pass, within `budget`. Answers `None` when no record needed it. A deadline counts `transform_skipped_deadline` and every other Kernel failure `transform_skipped_unavailable`; either skips the write, and the next committing pass catches up.
pub(crate) fn touch_session(
    store: &KernelStore,
    budget: &EvalBudget,
    project: &ProjectBinding,
    harness: &str,
    session_id: &str,
    at: i64,
) -> Option<LifecycleWrite> {
    match store.advance_use_outcome_last_transform(
        budget,
        project.digest(),
        harness,
        session_id,
        at,
    ) {
        Ok(0) => None,
        Ok(_) => Some(LifecycleWrite::TransformTouched),
        Err(error) => Some(skipped_touch(error)),
    }
}

/// Whether a live record of the session needs `last_transform_at` advanced to `at`, read within `budget`. A Kernel failure answers the skip [`touch_session`] would count for it.
pub(crate) fn touch_needed(
    store: &KernelStore,
    budget: &EvalBudget,
    project: &ProjectBinding,
    harness: &str,
    session_id: &str,
    at: i64,
) -> Result<bool, LifecycleWrite> {
    store
        .use_outcome_last_transform_is_stale(budget, project.digest(), harness, session_id, at)
        .map_err(skipped_touch)
}

/// The touch a serve job makes for its pass after its serve write, with a Kernel wait budget of its own: the staleness check and the update each spend what remains of it. Answers `None` when no live record needed the update.
pub(crate) fn touch_after_serve(
    store: &KernelStore,
    project: &ProjectBinding,
    harness: &str,
    session_id: &str,
    at: i64,
) -> Option<LifecycleWrite> {
    let budget = PassWriteBudget::default();
    match budget.spend(|budget| touch_needed(store, budget, project, harness, session_id, at)) {
        Ok(true) => {
            budget.spend(|budget| touch_session(store, budget, project, harness, session_id, at))
        }
        Ok(false) => None,
        Err(skipped) => Some(skipped),
    }
}

fn skipped_touch(error: KernelError) -> LifecycleWrite {
    match UseOutcomeWrite::from_error(error) {
        UseOutcomeWrite::Deadline => LifecycleWrite::TransformSkippedDeadline,
        _ => LifecycleWrite::TransformSkippedUnavailable,
    }
}

/// `PassWriteBudget` gives each operation the remaining `WRITE_BUDGET` and deducts its elapsed time. Transform passes hand serve writes to tracked jobs with separate waits.
#[derive(Debug)]
pub(crate) struct PassWriteBudget(Mutex<Duration>);

impl Default for PassWriteBudget {
    fn default() -> Self {
        Self(Mutex::new(WRITE_BUDGET))
    }
}

impl PassWriteBudget {
    pub(crate) fn spend<T>(&self, write: impl FnOnce(&EvalBudget) -> T) -> T {
        let remaining = *self.0.lock().expect("pass write budget mutex");
        let started = Instant::now();
        let outcome = write(&EvalBudget::new(Some(started + remaining), Arc::default()));
        let mut left = self.0.lock().expect("pass write budget mutex");
        *left = left.saturating_sub(started.elapsed());
        outcome
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
    length_prefixed_digest(
        REQUEST_DOMAIN,
        served_ids.iter().copied().chain([harness, role.as_str()]),
    )
}

fn length_prefixed_digest<'a>(domain: &[u8], fields: impl Iterator<Item = &'a str>) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    for field in fields {
        hash.update(field.len().to_string().as_bytes());
        hash.update(b":");
        hash.update(field.as_bytes());
        hash.update(b"\n");
    }
    format!("{:x}", hash.finalize())
}
