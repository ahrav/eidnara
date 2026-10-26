//! Records a frozen Canonical composition as a Kernel use-outcome serve: which decision objects one session was served.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{CommitIntent, KernelError, SessionRole, UseOutcomeSpec};
use sha2::{Digest, Sha256};

use crate::canonical_memory::CanonicalMemorySnapshot;
use crate::kernel_routes::{KernelOpenCoordinator, ProjectBinding};

const PRODUCER: &str = "eidnara-daemon/use-outcome";
const FAMILY: &str = "use-outcome";
const REQUEST_DOMAIN: &[u8] = b"eidnara-use-outcome-serve-v1\n";
/// Bounds the wait for the Kernel writer; the insert runs to completion once the writer is held.
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
        request_digest: request_digest(snapshot.served_ids(), serve.harness, session_role),
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
    let budget = EvalBudget::new(Some(Instant::now() + WRITE_BUDGET), Arc::default());
    Some(
        match store.commit_within_budget(&budget, intent, |envelope| {
            envelope.insert_use_outcome(spec)
        }) {
            Ok(receipt) if receipt.replayed => UseOutcomeWrite::Replayed,
            Ok(_) => UseOutcomeWrite::Written,
            Err(error) => UseOutcomeWrite::from_error(error),
        },
    )
}

fn request_digest(served_ids: &[String], harness: &str, role: SessionRole) -> String {
    let mut hash = Sha256::new();
    hash.update(REQUEST_DOMAIN);
    for field in served_ids
        .iter()
        .map(String::as_str)
        .chain([harness, role.as_str()])
    {
        hash.update(field.len().to_string().as_bytes());
        hash.update(b":");
        hash.update(field.as_bytes());
        hash.update(b"\n");
    }
    format!("{:x}", hash.finalize())
}
