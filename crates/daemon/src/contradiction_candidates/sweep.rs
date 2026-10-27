//! Space reclaim for candidate rows. Each run reads one page of at most
//! [`CANDIDATE_SWEEP_PAGE`] rows after an in-memory rowid watermark and
//! deletes the rows that can no longer be served. Readers re-prove liveness
//! at their own tip, so the sweep bounds the table's size and keeps no state
//! across a restart. The sweep reads the Kernel and writes only the memory
//! store.

use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{
    ClaimFactBounds, ClaimFacts, EvidenceLiveness, IdentifiedCommitReadTarget, KernelStore,
};
use memory_store::MemoryStore;
use memory_store::contradiction_candidates::{CandidateKey, StoredCandidate};

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

/// The sweep's rowid watermark, owned by the cleanup loop.
#[derive(Debug, Default)]
pub struct CandidateSweep {
    after_rowid: i64,
}

impl CandidateSweep {
    /// One run. Under a false `enabled` it returns before any table statement
    /// and changes no counter. With `kernel` absent it counts
    /// `sweep_skipped_kernel_unavailable` and reads nothing. Otherwise it
    /// examines one page and deletes rows whose Kernel incarnation or matcher
    /// digest is foreign, whose age exceeds [`MAX_CANDIDATE_AGE`], whose
    /// decision fails [`eligible`] at the run's tip, or whose descriptor
    /// evidence is not live there; a Kernel read that fails deletes nothing
    /// and counts the skip. The watermark advances past every page read and
    /// returns to the first row after the last page, so a table of `n` pages
    /// is covered in `n` runs.
    pub fn run(
        &mut self,
        store: &MemoryStore,
        kernel: Option<&KernelStore>,
        enabled: bool,
        counts: &Candidates,
        now_ms: i64,
    ) {
        if !enabled {
            return;
        }
        let Some(kernel) = kernel else {
            counts.add(Count::SweepSkippedKernelUnavailable, 1);
            return;
        };
        let budget = EvalBudget::new(Some(Instant::now() + SWEEP_BUDGET), Arc::default());
        let Ok(target) = kernel.capture_identified_commit_read_target_within_budget(&budget) else {
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
        let Some(dead) = dead_rows(kernel, &target, &rows, now_ms, &budget) else {
            counts.add(Count::SweepSkippedKernelUnavailable, 1);
            return;
        };
        let keys: Vec<CandidateKey<'_>> = rows
            .iter()
            .zip(&dead)
            .filter(|(_, dead)| **dead)
            .map(|(row, _)| CandidateKey {
                project_digest: &row.project_digest,
                evidence_id: &row.evidence_id,
                decision_object_id: &row.decision_object_id,
            })
            .collect();
        let gauge = match store.delete_candidates(&keys) {
            Ok(deleted) => {
                counts.add(Count::SweepDropped, deleted as u64);
                store.candidate_rows_and_bytes(None)
            }
            Err(error) => Err(error),
        };
        match gauge {
            Ok((_, bytes)) => counts.set(Count::RetainedBytes, bytes),
            Err(error) => {
                eprintln!("daemon: contradiction candidate sweep delete failed: {error}");
                counts.add(Count::StoreUnavailable, 1);
            }
        }
    }
}

/// Which of `rows` are dead at `target`, or `None` when a Kernel read failed.
fn dead_rows(
    kernel: &KernelStore,
    target: &IdentifiedCommitReadTarget,
    rows: &[StoredCandidate],
    now_ms: i64,
    budget: &EvalBudget,
) -> Option<Vec<bool>> {
    let max_age_ms = i64::try_from(MAX_CANDIDATE_AGE.as_millis()).expect("age fits");
    let mut dead: Vec<bool> = rows
        .iter()
        .map(|row| {
            row.kernel_incarnation_id != target.database_incarnation_id
                || row.matcher_digest != matcher_digest()
                || now_ms.saturating_sub(row.created_at_ms) > max_age_ms
        })
        .collect();
    let decisions: Vec<String> = rows
        .iter()
        .zip(&dead)
        .filter(|(_, dead)| !**dead)
        .map(|(row, _)| row.decision_object_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let bounds = ClaimFactBounds {
        max_claims: CLAIM_CHUNK,
        max_causal_payload_bytes: CLAIM_CAUSAL_PAYLOAD_BYTES,
    };
    let mut facts: HashMap<String, ClaimFacts> = HashMap::new();
    for chunk in decisions.chunks(CLAIM_CHUNK.get()) {
        let read = kernel
            .claim_facts_at_within_budget(chunk, target.target, bounds, budget)
            .ok()?;
        facts.extend(
            read.claims
                .into_iter()
                .map(|claim| (claim.object.object_id.clone(), claim)),
        );
    }
    let mut filters: HashMap<&str, Option<ScopeFilter>> = HashMap::new();
    let mut terms = stored_terms(kernel);
    for (row, dead) in rows.iter().zip(dead.iter_mut()) {
        if *dead {
            continue;
        }
        let Some(claim) = facts.get(&row.decision_object_id) else {
            *dead = true;
            continue;
        };
        let filter = filters
            .entry(&row.project_digest)
            .or_insert_with(|| ScopeFilter::for_digest(&row.project_digest));
        let names_project = match filter {
            Some(filter) => filter
                .matches(claim.decision.scope_id.as_deref(), &mut terms)
                .ok()?,
            None => false,
        };
        *dead = !eligible(claim, names_project);
    }
    let evidence: Vec<&str> = rows
        .iter()
        .zip(&dead)
        .filter(|(_, dead)| !**dead)
        .map(|(row, _)| row.evidence_id.as_str())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let standing: HashMap<&str, EvidenceLiveness> = evidence
        .iter()
        .copied()
        .zip(
            kernel
                .evidence_liveness_at_within_budget(&evidence, target.target, budget)
                .ok()?,
        )
        .collect();
    for (row, dead) in rows.iter().zip(dead.iter_mut()) {
        *dead |= standing
            .get(row.evidence_id.as_str())
            .is_some_and(|liveness| *liveness != EvidenceLiveness::Live);
    }
    Some(dead)
}
