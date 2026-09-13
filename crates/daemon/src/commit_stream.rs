//! One bounded walk over a registered consumer's complete commits toward a captured target, shared by every daemon consumer of the kernel commit log.

use kernel::applicability::EvalBudget;
use kernel::{
    CommitPage, CommitPageBounds, CommitReadError, CommitReadIncarnation, CommitReadRequest,
    KernelError, KernelStore, PageEnd,
};

/// Why a walk stopped before its target. Nothing durable moved for the refused page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitStreamBlocked {
    /// The kernel rejects negative times at acknowledgement, so a walk refuses them before publishing an unacknowledgeable page.
    NegativeTime {
        now: i64,
    },
    Read(CommitReadError),
    OversizedCommit {
        commit_seq: i64,
        rows: usize,
        payload_bytes: u64,
    },
    /// The commit log holds no complete commit between the applied prefix and the captured target.
    TargetUnreachable {
        after: i64,
        target: i64,
    },
}

/// One consumer's walk over `(after, target]` under a captured incarnation.
#[derive(Debug, Clone)]
pub(crate) struct CommitWalk<'a> {
    /// `None` reads every page through the kernel's unbudgeted entry point.
    pub(crate) budget: Option<EvalBudget>,
    pub(crate) consumer_id: &'a str,
    pub(crate) incarnation: CommitReadIncarnation,
    /// Unix-epoch milliseconds the pages are acknowledged at.
    pub(crate) now: i64,
    /// The applied prefix the walk starts after.
    pub(crate) after: i64,
    pub(crate) target: i64,
    pub(crate) bounds: CommitPageBounds,
}

/// Reads the walk one page at a time and hands each page to `on_page` before reading the next. The first refusal, from the read or from `on_page`, ends the walk.
///
/// # Errors
///
/// Returns the read's refusal as [`CommitStreamBlocked`], a kernel failure as [`KernelError`], or whatever `on_page` returns.
pub(crate) fn drive_commit_pages<E>(
    kernel: &KernelStore,
    walk: CommitWalk<'_>,
    mut on_page: impl FnMut(&CommitPage) -> Result<(), E>,
) -> Result<(), E>
where
    E: From<CommitStreamBlocked> + From<KernelError>,
{
    let CommitWalk {
        budget,
        consumer_id,
        incarnation,
        now,
        mut after,
        target,
        bounds,
    } = walk;
    if now < 0 {
        return Err(CommitStreamBlocked::NegativeTime { now }.into());
    }
    while after < target {
        let request = CommitReadRequest {
            consumer_id: consumer_id.to_owned(),
            incarnation,
            after_commit: after,
            through_commit: target,
        };
        let page = match &budget {
            Some(budget) => kernel.read_complete_commits_within_budget(budget, &request, bounds),
            None => kernel.read_complete_commits(&request, bounds),
        }
        .map_err(|error| match error {
            CommitReadError::Kernel(error) => E::from(error),
            error => CommitStreamBlocked::Read(error).into(),
        })?;
        let last = match page.end {
            PageEnd::Oversized {
                commit_seq,
                rows,
                payload_bytes,
            } => {
                return Err(CommitStreamBlocked::OversizedCommit {
                    commit_seq,
                    rows,
                    payload_bytes,
                }
                .into());
            }
            PageEnd::Exhausted => true,
            PageEnd::Deferred { .. } => false,
        };
        // The target is a commit the kernel had at capture, so an empty page or an exhausted page below it means the commit log lost commits.
        if page.commits.is_empty() || (last && page.through < target) {
            return Err(CommitStreamBlocked::TargetUnreachable { after, target }.into());
        }
        on_page(&page)?;
        after = page.through;
    }
    Ok(())
}

/// An acknowledgement that failed this way may still have committed, because the failure can strike after the kernel's COMMIT or while waiting for its writer, so the durable checkpoint decides.
/// `Io` leaves the acknowledgement outcome unknown, so callers must resolve it before retrying.
/// `Deadline` is a definite refusal under the contract of [`KernelStore::acknowledge_through_source_hold_within_budget`].
pub(crate) fn outcome_unknown(error: KernelError) -> bool {
    matches!(
        error,
        KernelError::Busy | KernelError::Held | KernelError::Io
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_is_a_definite_refusal_not_an_unknown_acknowledgement() {
        assert!(!outcome_unknown(KernelError::Deadline));
        assert!(outcome_unknown(KernelError::Busy));
        assert!(outcome_unknown(KernelError::Held));
        assert!(outcome_unknown(KernelError::Io));
        assert!(!outcome_unknown(KernelError::FenceLost));
        assert!(!outcome_unknown(KernelError::InvalidCheckpoint));
    }
}
