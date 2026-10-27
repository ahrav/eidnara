//! The candidate pass: once a publication job ends `Completed` with published
//! units and has released the capture slot and commit gate, one pass scores
//! each published descriptor against the project's eligible `memory`
//! decisions at one Kernel tip and writes at most K rows per descriptor and M
//! per pass. One pass runs per store; an offer while one runs is refused. The
//! pass reads the Kernel and writes only the memory store.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{ClaimFactBounds, KernelStore, Surface};
use memory_store::MemoryStore;
use memory_store::contradiction_candidates::{CandidateCaps, CandidateScore, DescriptorCandidates};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;

use super::eligibility::eligible;
use super::scorer::{
    CANDIDATES_PER_DESCRIPTOR, CandidateInput, DescriptorTerms, SummaryScore, TopCandidates,
    matcher_digest,
};
use super::{CLAIM_CAUSAL_PAYLOAD_BYTES, Candidates, Count};
use crate::canonical_memory::MEMORY_DOMAIN_ID;
use crate::kernel_routes::KernelOpenCoordinator;
use crate::kernel_routes::project::{ProjectBinding, ScopeFilter, stored_terms};
use crate::kernel_routes::read::{MAX_READ_OBJECT_IDS, NewestRows};
use crate::source_capture::SettledPublication;

/// Rows one pass writes across its descriptors, in publish order (M).
pub const CANDIDATES_PER_PASS: usize = 64;
/// Eligible decisions one pass scores; the newest are kept.
pub const MAX_CANDIDATE_DECISIONS: usize = 4096;
/// Summary payload bytes one pass loads.
pub const MAX_CANDIDATE_SUMMARY_BYTES: u64 = 4 * 1024 * 1024;
/// Rows the memory store keeps for one project.
pub const MAX_CANDIDATE_ROWS_PER_PROJECT: usize = 4096;
/// The pass's deadline from its start.
pub const CANDIDATE_PASS_BUDGET: Duration = Duration::from_secs(10);
/// Summaries one descriptor scores between budget checks.
const SUMMARIES_PER_CHECK: usize = 256;
/// Visible rows the scope filter judges between budget checks.
const ROWS_PER_CHECK: usize = 256;
const CLAIM_CHUNK: NonZeroUsize = NonZeroUsize::new(64).expect("nonzero");

/// The pass's bounds; production uses [`PassBounds::default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PassBounds {
    pub(crate) rows_per_pass: usize,
    pub(crate) decisions: usize,
    pub(crate) summary_bytes: u64,
    pub(crate) rows_per_project: usize,
    pub(crate) budget: Duration,
}

impl Default for PassBounds {
    fn default() -> Self {
        Self {
            rows_per_pass: CANDIDATES_PER_PASS,
            decisions: MAX_CANDIDATE_DECISIONS,
            summary_bytes: MAX_CANDIDATE_SUMMARY_BYTES,
            rows_per_project: MAX_CANDIDATE_ROWS_PER_PROJECT,
            budget: CANDIDATE_PASS_BUDGET,
        }
    }
}

/// One descriptor a pass scores: the exact text the Kernel retained, and its evidence row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Descriptor {
    pub(crate) evidence_id: String,
    pub(crate) text: String,
}

/// An admitted pass: it holds the store's pass permit and the publication's charge.
pub(crate) struct CandidatePass {
    kernel: Arc<KernelOpenCoordinator>,
    store: Arc<MemoryStore>,
    cancel: CancellationToken,
    settled: SettledPublication,
    active: ActivePass,
}

/// Counts the pass as active while the permit lives, so a panicking pass releases both.
struct ActivePass {
    counts: Arc<Candidates>,
    _permit: OwnedSemaphorePermit,
}

impl ActivePass {
    fn new(counts: &Arc<Candidates>, permit: OwnedSemaphorePermit) -> Self {
        counts.add(Count::Active, 1);
        Self {
            counts: Arc::clone(counts),
            _permit: permit,
        }
    }
}

impl Drop for ActivePass {
    fn drop(&mut self) {
        self.counts.sub(Count::Active, 1);
    }
}

impl Candidates {
    /// Takes the store's pass permit for `settled`, or counts `refused_pass_busy` and returns `None` while a pass runs.
    pub(crate) fn offer(
        self: &Arc<Self>,
        settled: SettledPublication,
        kernel: Arc<KernelOpenCoordinator>,
        store: Arc<MemoryStore>,
        cancel: CancellationToken,
    ) -> Option<CandidatePass> {
        self.add(Count::Offered, 1);
        let Ok(permit) = Arc::clone(&self.slot).try_acquire_owned() else {
            self.add(Count::RefusedPassBusy, 1);
            return None;
        };
        Some(CandidatePass {
            kernel,
            store,
            cancel,
            settled,
            active: ActivePass::new(self, permit),
        })
    }
}

impl CandidatePass {
    /// Runs the pass on the blocking pool and returns once it physically completes. The blocking closure records the pass's end, a panic included, and its timing before it releases the permit, so an idle permit means those counters are final. Cancellation raises the budget's interrupt; the pass stops at its next check. Between checks it runs one budgeted Kernel read, up to 256 scope-term reads, the unbudgeted visible-decision and payload-size reads, 256 summaries of one descriptor, or one row write.
    pub(crate) async fn run(self) -> Count {
        let counts = Arc::clone(&self.active.counts);
        counts.add(Count::Admitted, 1);
        let interrupt = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&interrupt);
        let cancel = self.cancel.clone();
        let mut join = tokio::task::spawn_blocking(move || self.finish(&interrupt));
        let joined = tokio::select! {
            joined = &mut join => joined,
            () = cancel.cancelled() => {
                raised.store(true, Ordering::Release);
                join.await
            }
        };
        match joined {
            Ok(end) => end,
            Err(error) => {
                // A closure the runtime dropped before it ran was stopped by shutdown, not a panic.
                let end = if error.is_panic() {
                    Count::WorkerPanicked
                } else {
                    Count::BudgetExhausted
                };
                counts.add(end, 1);
                end
            }
        }
    }

    fn finish(mut self, interrupt: &Arc<AtomicBool>) -> Count {
        let started = Instant::now();
        let counts = Arc::clone(&self.active.counts);
        // The texts move out; the charge stays with `settled` until the pass ends.
        let descriptors: Vec<Descriptor> = std::mem::take(&mut self.settled.units)
            .into_iter()
            .map(|(text, published)| Descriptor {
                evidence_id: published.evidence_id,
                text,
            })
            .collect();
        let scored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            {
                let hook = counts.before_pass.lock().unwrap().clone();
                if let Some(hook) = hook {
                    hook();
                }
            }
            let deadline = Instant::now() + counts.bounds().budget;
            let budget = EvalBudget::new(Some(deadline), Arc::clone(interrupt));
            match self.kernel.kernel_store() {
                Ok(kernel) => score_publication(
                    &kernel,
                    &self.store,
                    &self.settled.project,
                    &descriptors,
                    &counts,
                    &budget,
                    crate::now_ms(),
                ),
                Err(_) => {
                    counts.add(Count::DescriptorsUnscored, descriptors.len() as u64);
                    counts.add(Count::KernelUnavailable, 1);
                    Count::KernelUnavailable
                }
            }
        }));
        let end = scored.unwrap_or_else(|_| {
            counts.add(Count::WorkerPanicked, 1);
            Count::WorkerPanicked
        });
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        counts.set(Count::PassMsLast, elapsed);
        counts.max(Count::PassMsMax, elapsed);
        end
    }
}

/// How a stopped budget ends the pass: `deadline` once its deadline passed, `budget_exhausted` for the daemon's cancel.
fn stop(budget: &EvalBudget) -> Option<Count> {
    budget.is_exhausted().then(|| {
        if budget
            .deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Count::Deadline
        } else {
            Count::BudgetExhausted
        }
    })
}

/// Scores `descriptors` in publish order against the eligible decisions of `project` at one Kernel tip and writes each descriptor's rows. A stopped budget leaves the rows of a publish-order prefix: a descriptor not fully scored writes nothing and counts `descriptors_unscored` with every later one.
pub(crate) fn score_publication(
    kernel: &KernelStore,
    store: &MemoryStore,
    project: &ProjectBinding,
    descriptors: &[Descriptor],
    counts: &Candidates,
    budget: &EvalBudget,
    now_ms: i64,
) -> Count {
    let end = score_descriptors(kernel, store, project, descriptors, counts, budget, now_ms);
    counts.add(end, 1);
    end
}

fn score_descriptors(
    kernel: &KernelStore,
    store: &MemoryStore,
    project: &ProjectBinding,
    descriptors: &[Descriptor],
    counts: &Candidates,
    budget: &EvalBudget,
    now_ms: i64,
) -> Count {
    let bounds = &counts.bounds();
    let unscored = |from: usize, end: Count| {
        counts.add(
            Count::DescriptorsUnscored,
            (descriptors.len() - from) as u64,
        );
        end
    };
    if let Some(end) = stop(budget) {
        return unscored(0, end);
    }
    let Ok(target) = kernel.capture_identified_commit_read_target_within_budget(budget) else {
        return unscored(0, stop(budget).unwrap_or(Count::KernelUnavailable));
    };
    let tip = target.target.through_commit;
    let summaries = match candidate_summaries(kernel, project, &target, counts, bounds, budget) {
        Ok(summaries) => summaries,
        Err(end) => return unscored(0, end),
    };
    let caps = CandidateCaps {
        per_descriptor: CANDIDATES_PER_DESCRIPTOR,
        per_project: bounds.rows_per_project,
    };
    let mut written = 0;
    let mut unscorable_counted = false;
    for (index, descriptor) in descriptors.iter().enumerate() {
        if let Some(end) = stop(budget) {
            return unscored(index, end);
        }
        #[cfg(test)]
        {
            let hook = counts.before_descriptor.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(index);
            }
        }
        let Ok(terms) = DescriptorTerms::new(&descriptor.text) else {
            counts.add(Count::DescriptorsRefusedOversize, 1);
            continue;
        };
        let mut top = TopCandidates::default();
        let mut unscorable = 0;
        for (scanned, (object_id, decision_id, summary)) in summaries.iter().enumerate() {
            if scanned % SUMMARIES_PER_CHECK == 0
                && let Some(end) = stop(budget)
            {
                return unscored(index, end);
            }
            let input = CandidateInput {
                decision_object_id: object_id,
                decision_id,
                summary,
            };
            if top.record(&terms, input) == SummaryScore::Unscorable {
                unscorable += 1;
            }
        }
        // Whether a summary is unscorable depends on the summary alone, so the first scored descriptor counts each one once.
        if !unscorable_counted {
            counts.add(Count::SummariesUnscorable, unscorable);
            unscorable_counted = true;
        }
        if let Some(end) = stop(budget) {
            return unscored(index, end);
        }
        counts.add(Count::DescriptorsAnalyzed, 1);
        let mut rows = top.into_vec();
        let room = bounds.rows_per_pass.saturating_sub(written);
        // A pass at its cap leaves the rows the descriptor already holds; an empty write would replace them.
        if room == 0 && !rows.is_empty() {
            counts.add(Count::DroppedAtCap, rows.len() as u64);
            continue;
        }
        if rows.len() > room {
            counts.add(Count::DroppedAtCap, (rows.len() - room) as u64);
            rows.truncate(room);
        }
        let scores: Vec<CandidateScore<'_>> = rows
            .iter()
            .map(|row| CandidateScore {
                decision_object_id: &row.decision_object_id,
                decision_id: &row.decision_id,
                matched_terms: u32::try_from(row.matched_terms).unwrap_or(u32::MAX),
                decision_terms: u32::try_from(row.decision_terms).unwrap_or(u32::MAX),
            })
            .collect();
        let write = DescriptorCandidates {
            project_digest: project.digest(),
            evidence_id: &descriptor.evidence_id,
            publish_tip: tip,
            kernel_incarnation_id: &target.database_incarnation_id,
            matcher_digest: matcher_digest(),
            created_at_ms: now_ms,
            rows: &scores,
        };
        match store.replace_candidates_for_descriptor(&write, caps) {
            Ok(outcome) => {
                written += outcome.written;
                counts.add(Count::RowsWritten, outcome.written as u64);
                counts.add(Count::DroppedAtCap, outcome.dropped_at_cap as u64);
            }
            Err(error) => {
                eprintln!("daemon: contradiction candidate write failed: {error}");
                return unscored(index + 1, Count::StoreUnavailable);
            }
        }
    }
    Count::Completed
}

/// The `(object id, decision id, summary)` of each eligible decision the pass scores, newest first: the explicit-search ids of the project's `memory` decisions at the tip, cut to the newest `bounds.decisions`, judged eligible through claim facts, and loaded until their payloads would pass `bounds.summary_bytes`.
fn candidate_summaries(
    kernel: &KernelStore,
    project: &ProjectBinding,
    target: &kernel::IdentifiedCommitReadTarget,
    counts: &Candidates,
    bounds: &PassBounds,
    budget: &EvalBudget,
) -> Result<Vec<(String, String, String)>, Count> {
    let failed = |_| stop(budget).unwrap_or(Count::KernelUnavailable);
    let tip = target.target.through_commit;
    let visible = kernel
        .visible_domain_decisions_as_of_in_scope(
            Surface::ExplicitSearch,
            tip,
            MEMORY_DOMAIN_ID,
            Some(project.scope_term()),
        )
        .map_err(failed)?;
    let mut filter = ScopeFilter::new(project);
    let mut terms = stored_terms(kernel);
    let mut newest = NewestRows::new(bounds.decisions);
    for (judged, row) in visible.rows.into_iter().enumerate() {
        if judged % ROWS_PER_CHECK == 0
            && let Some(end) = stop(budget)
        {
            return Err(end);
        }
        if filter
            .matches(row.scope_id.as_deref(), &mut terms)
            .map_err(failed)?
        {
            newest.push(row);
        }
    }
    let (rows, truncated) = newest.finish();
    if truncated {
        counts.add(Count::CandidatesTruncated, 1);
    }
    let ids: Vec<String> = rows.into_iter().map(|row| row.object.object_id).collect();
    let claims = ClaimFactBounds {
        max_claims: CLAIM_CHUNK,
        max_causal_payload_bytes: CLAIM_CAUSAL_PAYLOAD_BYTES,
    };
    let mut kept = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(CLAIM_CHUNK.get()) {
        if let Some(end) = stop(budget) {
            return Err(end);
        }
        let read = kernel
            .claim_facts_at_within_budget(chunk, target.target, claims, budget)
            .map_err(|_| stop(budget).unwrap_or(Count::KernelUnavailable))?;
        // The scope filter above already kept only rows whose scope names the project.
        let standing: HashMap<&str, bool> = read
            .claims
            .iter()
            .map(|facts| (facts.object.object_id.as_str(), eligible(facts, true)))
            .collect();
        kept.extend(
            chunk
                .iter()
                .filter(|id| standing.get(id.as_str()).copied().unwrap_or(false))
                .cloned(),
        );
    }
    if let Some(end) = stop(budget) {
        return Err(end);
    }
    let sizes: HashMap<String, u64> = kernel
        .decision_payload_sizes_as_of(&kept, tip)
        .map_err(failed)?
        .into_iter()
        .collect();
    let mut loaded_bytes = 0_u64;
    let mut cut = 0_u64;
    let mut load = Vec::with_capacity(kept.len());
    for id in kept {
        let size = sizes.get(&id).copied().unwrap_or(0);
        // The cut is a prefix of the newest-first order: once one payload passes the budget, every older one is cut too.
        if cut > 0 || loaded_bytes.saturating_add(size) > bounds.summary_bytes {
            cut = cut.saturating_add(size);
        } else {
            loaded_bytes += size;
            load.push(id);
        }
    }
    counts.add(Count::SummariesCutBytes, cut);
    let mut summaries = Vec::with_capacity(load.len());
    for chunk in load.chunks(MAX_READ_OBJECT_IDS) {
        if let Some(end) = stop(budget) {
            return Err(end);
        }
        let mut rows: HashMap<String, kernel::DecisionRow> = kernel
            .decisions_for_objects_as_of_within_budget(chunk, tip, budget)
            .map_err(failed)?
            .into_iter()
            .map(|row| (row.object_id.clone(), row))
            .collect();
        for id in chunk {
            if let Some(row) = rows.remove(id) {
                summaries.push((row.object_id, row.decision_id, row.payload.summary));
            }
        }
    }
    counts.add(Count::SummariesLoaded, summaries.len() as u64);
    Ok(summaries)
}
