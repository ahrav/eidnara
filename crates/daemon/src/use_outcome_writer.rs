//! Records a frozen Canonical composition as a Kernel use-outcome serve: which decision objects one session was served. Also writes the record lifecycle: `session.delete` marks a session's live records deleted, and a committing pass advances their `last_transform_at`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{CommitIntent, KernelError, KernelStore, SessionRole, UseOutcomeSpec};
use sha2::{Digest, Sha256};

use crate::canonical_memory::CanonicalMemorySnapshot;
use crate::kernel_routes::{KernelOpenCoordinator, ProjectBinding};

const PRODUCER: &str = "eidnara-daemon/use-outcome";
const FAMILY: &str = "use-outcome";
const REQUEST_DOMAIN: &[u8] = b"eidnara-use-outcome-serve-v1\n";
const DELETE_PRODUCER: &str = "eidnara-daemon/use-outcome-delete";
const DELETE_FAMILY: &str = "use-outcome-delete";
const DELETE_REQUEST_DOMAIN: &[u8] = b"eidnara-use-outcome-delete-v1\n";
/// Bounds one pass's total wait for Kernel connections across its writes; a write runs to completion once its connection is held.
const WRITE_BUDGET: Duration = Duration::from_millis(250);

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

#[derive(Debug, Default)]
pub(crate) struct UseOutcomeCounts([AtomicU64; UseOutcomeWrite::ALL.len()]);

impl UseOutcomeCounts {
    pub(crate) fn record(&self, outcome: UseOutcomeWrite) {
        self.0[outcome as usize].fetch_add(1, Ordering::Relaxed);
        if !matches!(
            outcome,
            UseOutcomeWrite::Written | UseOutcomeWrite::Replayed
        ) {
            eprintln!("daemon: use-outcome serve write: {}", outcome.name());
        }
    }

    #[cfg(test)]
    pub(crate) fn get(&self, outcome: UseOutcomeWrite) -> u64 {
        self.0[outcome as usize].load(Ordering::Relaxed)
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

/// `session.delete`'s use-outcome step: one envelope that sets `deleted_at = now` on every live record of the session, selected under the writer. An envelope that marks nothing rolls back, so a session with no live record commits nothing. The key carries a fresh attempt nonce, so a receipt never answers a later delete as a replay; idempotence rests on the live selection, and a record already marked keeps its first timestamp.
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
    let operation_key = project
        .operation_key(DELETE_FAMILY, &format!("{session_id}:{nonce}"))
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
        Err(error) => Some(match UseOutcomeWrite::from_error(error) {
            UseOutcomeWrite::Deadline => LifecycleWrite::TransformSkippedDeadline,
            _ => LifecycleWrite::TransformSkippedUnavailable,
        }),
    }
}

/// The Kernel wait one pass may spend across its serve and touch writes. Each write gets what remains and spends its elapsed time, so a pass waits on the Kernel for at most `WRITE_BUDGET` in total.
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
/// `kernel_unavailable` from a commit can follow a writer wait, and after a post-commit `Io` or `Inconclusive`
/// the record may exist; a later write at the same key replays it.
pub(crate) fn record_serve(
    kernel: &KernelOpenCoordinator,
    budget: &PassWriteBudget,
    serve: &Serve<'_>,
) -> Option<UseOutcomeWrite> {
    let store = kernel.kernel_store().ok()?;
    let snapshot = serve.snapshot;
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
    let intent = CommitIntent {
        producer: PRODUCER.to_string(),
        operation_key,
        request_digest: length_prefixed_digest(
            REQUEST_DOMAIN,
            snapshot
                .served_ids()
                .iter()
                .map(String::as_str)
                .chain([serve.harness, session_role.as_str()]),
        ),
        actor: "daemon".to_string(),
        cause: "use outcome served".to_string(),
    };
    let spec = UseOutcomeSpec {
        project_digest: serve.project.digest().to_string(),
        harness: serve.harness.to_string(),
        session_id: serve.session_id.to_string(),
        served_set_digest: digest.to_string(),
        composition_revision: snapshot.revision(),
        served_commit_seq: snapshot.known_as_of(),
        session_role,
        recorded_at: serve.recorded_at,
        decision_object_ids: snapshot.served_ids().to_vec(),
    };
    Some(
        match budget.spend(|budget| {
            store.commit_within_budget(budget, intent, |envelope| envelope.insert_use_outcome(spec))
        }) {
            Ok(receipt) if receipt.replayed => UseOutcomeWrite::Replayed,
            Ok(_) => UseOutcomeWrite::Written,
            Err(error) => UseOutcomeWrite::from_error(error),
        },
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
