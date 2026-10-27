//! Space reclaim for candidate rows. Each run reads one page of at most
//! [`CANDIDATE_SWEEP_PAGE`] rows after an in-memory rowid watermark and
//! deletes the rows that can no longer be served. Readers re-prove liveness
//! at their own tip, so the sweep bounds the table's size and keeps no state
//! across a restart. The sweep reads the Kernel and writes only the memory
//! store.

use std::collections::HashMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{
    ClaimFactBounds, ClaimFacts, EvidenceLiveness, IdentifiedCommitReadTarget, KernelError,
    KernelStore,
};
use memory_store::MemoryStore;
use memory_store::contradiction_candidates::StoredCandidate;

use super::eligibility::eligible;
use super::scorer::matcher_digest;
use super::{Candidates, Count};
use crate::kernel_routes::project::{ScopeFilter, stored_terms};

/// Rows one sweep run examines.
pub const CANDIDATE_SWEEP_PAGE: usize = 256;
/// A row older than this drops whatever the Kernel says: the retention of the
/// harness-capture evidence a descriptor's row cites.
pub const MAX_CANDIDATE_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The Kernel reads of one run stop at this deadline.
const SWEEP_BUDGET: Duration = Duration::from_secs(2);
/// Decisions per claim-facts read.
const CLAIM_CHUNK: NonZeroUsize = NonZeroUsize::new(64).expect("nonzero");
const CLAIM_CAUSAL_PAYLOAD_BYTES: NonZeroU64 = NonZeroU64::new(1024 * 1024).expect("nonzero");

// A page's distinct evidence ids fit one liveness read.
const _: () = assert!(CANDIDATE_SWEEP_PAGE <= kernel::MAX_EVIDENCE_LIVENESS_IDS);

/// The sweep's rowid watermark, owned by the cleanup loop.
#[derive(Debug, Default)]
pub struct CandidateSweep {
    after_rowid: i64,
}

/// A row's standing after one run's reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Dead,
    /// Not yet judged dead; a Kernel read that fails leaves its rows here.
    Kept,
}

impl CandidateSweep {
    /// One run. Under a false `enabled` it returns before any table statement
    /// and changes no counter. With `kernel` absent it counts
    /// `sweep_skipped_kernel_unavailable` and reads nothing. Otherwise it
    /// reads one page, then captures the Kernel tip, so every row on the page
    /// was written at or below that tip. It deletes rows whose Kernel
    /// incarnation or matcher digest is foreign or whose age exceeds
    /// [`MAX_CANDIDATE_AGE`] without a Kernel read, then rows whose decision
    /// fails [`eligible`] at the tip or whose descriptor evidence is not live
    /// there. A Kernel read that fails keeps the rows it would have judged and
    /// counts one skip. A row rewritten after the page read keeps its newer
    /// values. `cancelled` is read before each Kernel read and before the
    /// delete; once it fires the run deletes nothing. The watermark advances
    /// past every page read and returns to the first row after the last page,
    /// so a table of `n` pages is covered in `n` runs. `retained_bytes` is
    /// refreshed when the watermark returns to the first row.
    pub fn run(
        &mut self,
        store: &MemoryStore,
        kernel: Option<&KernelStore>,
        enabled: bool,
        counts: &Candidates,
        now_ms: i64,
        cancelled: &dyn Fn() -> bool,
    ) {
        if !enabled {
            return;
        }
        let Some(kernel) = kernel else {
            counts.add(Count::SweepSkippedKernelUnavailable, 1);
            return;
        };
        let mut page = match store.candidate_sweep_page(self.after_rowid, CANDIDATE_SWEEP_PAGE + 1)
        {
            Ok(page) => page,
            Err(error) => {
                eprintln!("daemon: contradiction candidate sweep read failed: {error}");
                counts.add(Count::StoreUnavailable, 1);
                return;
            }
        };
        let more = page.len() > CANDIDATE_SWEEP_PAGE;
        page.truncate(CANDIDATE_SWEEP_PAGE);
        self.after_rowid = match page.last() {
            Some((rowid, _)) if more => *rowid,
            _ => 0,
        };
        counts.add(Count::SweepExamined, page.len() as u64);
        let rows: Vec<StoredCandidate> = page.into_iter().map(|(_, row)| row).collect();
        let max_age_ms = i64::try_from(MAX_CANDIDATE_AGE.as_millis()).expect("age fits");
        let standing: Vec<Standing> = rows
            .iter()
            .map(|row| {
                if row.matcher_digest != matcher_digest()
                    || now_ms.saturating_sub(row.created_at_ms) > max_age_ms
                {
                    Standing::Dead
                } else {
                    Standing::Kept
                }
            })
            .collect();
        let budget = EvalBudget::new(Some(Instant::now() + SWEEP_BUDGET), Arc::default());
        let judged = match kernel.capture_identified_commit_read_target_within_budget(&budget) {
            Ok(target) => judge(kernel, &target, &rows, standing, &budget, cancelled),
            Err(_) => Judged {
                standing,
                read_failed: true,
            },
        };
        if judged.read_failed {
            counts.add(Count::SweepSkippedKernelUnavailable, 1);
        }
        if cancelled() {
            return;
        }
        let dead: Vec<&StoredCandidate> = rows
            .iter()
            .zip(&judged.standing)
            .filter(|(_, standing)| **standing == Standing::Dead)
            .map(|(row, _)| row)
            .collect();
        let refreshed = store.delete_candidates_as_read(&dead).and_then(|deleted| {
            counts.add(Count::SweepDropped, deleted as u64);
            if self.after_rowid == 0 {
                let (_, bytes) = store.candidate_rows_and_bytes(None)?;
                counts.set(Count::RetainedBytes, bytes);
            }
            Ok(())
        });
        if let Err(error) = refreshed {
            eprintln!("daemon: contradiction candidate sweep delete or gauge read failed: {error}");
            counts.add(Count::StoreUnavailable, 1);
        }
    }
}

struct Judged {
    standing: Vec<Standing>,
    read_failed: bool,
}

/// The distinct values `key` takes over the rows still [`Standing::Kept`], in
/// ascending order, so each Kernel read walks its indexes in key order.
fn kept_unique<'a, T: Ord>(
    rows: &'a [StoredCandidate],
    standing: &[Standing],
    key: impl Fn(&'a StoredCandidate) -> T,
) -> Vec<T> {
    let mut values: Vec<T> = rows
        .iter()
        .zip(standing)
        .filter(|(_, standing)| **standing == Standing::Kept)
        .map(|(row, _)| key(row))
        .collect();
    values.sort_unstable();
    values.dedup();
    values
}

/// Each row's standing at `target`, from the standing judged without the
/// Kernel. A row written after `target` stays kept. A claim chunk the Kernel
/// refuses is read again one decision at a time, so one decision this build
/// cannot read keeps only its own rows.
fn judge(
    kernel: &KernelStore,
    target: &IdentifiedCommitReadTarget,
    rows: &[StoredCandidate],
    mut standing: Vec<Standing>,
    budget: &EvalBudget,
    cancelled: &dyn Fn() -> bool,
) -> Judged {
    for (row, standing) in rows.iter().zip(standing.iter_mut()) {
        if row.kernel_incarnation_id != target.database_incarnation_id {
            *standing = Standing::Dead;
        }
    }
    let mut read_failed = false;
    let bounds = ClaimFactBounds {
        max_claims: CLAIM_CHUNK,
        max_causal_payload_bytes: CLAIM_CAUSAL_PAYLOAD_BYTES,
    };
    let decisions = kept_unique(rows, &standing, |row| row.decision_object_id.clone());
    let mut facts: HashMap<String, Option<ClaimFacts>> = HashMap::new();
    for chunk in decisions.chunks(CLAIM_CHUNK.get()) {
        if cancelled() {
            return Judged {
                standing,
                read_failed,
            };
        }
        let mut read = |ids: &[String]| match kernel.claim_facts_at_within_budget(
            ids,
            target.target,
            bounds,
            budget,
        ) {
            Ok(read) => {
                facts.extend(ids.iter().map(|id| (id.clone(), None)));
                facts.extend(
                    read.claims
                        .into_iter()
                        .map(|claim| (claim.object.object_id.clone(), Some(claim))),
                );
                true
            }
            Err(_) => false,
        };
        if !read(chunk) {
            for id in chunk {
                read_failed |= !read(std::slice::from_ref(id));
            }
        }
    }
    let mut filters: HashMap<&str, Option<ScopeFilter>> = HashMap::new();
    let mut stored = stored_terms(kernel);
    let mut terms = |scope: &str| {
        if budget.is_exhausted() {
            Err(KernelError::Deadline)
        } else {
            stored(scope)
        }
    };
    for (row, standing) in rows.iter().zip(standing.iter_mut()) {
        if *standing == Standing::Dead || row.publish_tip > target.target.through_commit {
            continue;
        }
        let claim = match facts.get(&row.decision_object_id) {
            Some(Some(claim)) => claim,
            Some(None) => {
                *standing = Standing::Dead;
                continue;
            }
            None => continue,
        };
        let filter = filters
            .entry(&row.project_digest)
            .or_insert_with(|| ScopeFilter::for_digest(&row.project_digest));
        let names_project = match filter {
            Some(filter) => match filter.matches(claim.decision.scope_id.as_deref(), &mut terms) {
                Ok(verdict) => verdict,
                Err(_) => {
                    read_failed = true;
                    continue;
                }
            },
            None => false,
        };
        if !eligible(claim, names_project) {
            *standing = Standing::Dead;
        }
    }
    let evidence = kept_unique(rows, &standing, |row| row.evidence_id.as_str());
    if cancelled() {
        return Judged {
            standing,
            read_failed,
        };
    }
    let liveness = match kernel.evidence_liveness_at_within_budget(&evidence, target.target, budget)
    {
        Ok(liveness) => liveness,
        Err(_) => {
            return Judged {
                standing,
                read_failed: true,
            };
        }
    };
    let live: HashMap<&str, EvidenceLiveness> = evidence.into_iter().zip(liveness).collect();
    for (row, standing) in rows.iter().zip(standing.iter_mut()) {
        if row.publish_tip <= target.target.through_commit
            && live
                .get(row.evidence_id.as_str())
                .is_some_and(|liveness| *liveness != EvidenceLiveness::Live)
        {
            *standing = Standing::Dead;
        }
    }
    Judged {
        standing,
        read_failed,
    }
}
