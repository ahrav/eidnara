//! Each occurrence keeps its best probe; canonical eligibility is judged in bounded batches before acceptance; accepted contributions are re-judged before return.
//! This module does not read payload bytes; each contribution contains an occurrence identifier and raw FTS rank.
//!
//! Ordering is one comparator throughout: lower raw rank first, then occurrence identifier bytes ascending.
//! An occurrence hit by several probes keeps the lowest rank, and among equal ranks the lowest probe ordinal, so duplicate or permuted probes leave the ranking unchanged.
//! A probe repeated later in the request therefore cannot change the ranking, and the engine runs it only once; the repeat replays the first run's counters.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CommitReadIncarnation, EgressSnapshot, EligibilityVerdict, KernelError, KernelStore,
    MAX_ELIGIBILITY_CANDIDATES,
};
use storage::GuardedConn;

use super::Probe;
use crate::ProjectionError;
pub use crate::eligibility::Authority;
use crate::eligibility::{
    AuthorityMoved, Disposition, EligibilityReport, OccurrenceCandidate, judge_tracked,
    tally_exclusion,
};
use crate::scan::ScanStop;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrievalBounds {
    /// Probes one request may run; a request compiling to more is refused before any runs.
    pub max_probes: NonZeroUsize,
    /// Rows one probe may return from the engine, taken in comparator order.
    pub scan_rows: NonZeroUsize,
    /// A request accepts at most [`MAX_ELIGIBILITY_CANDIDATES`] occurrences so final re-judgment fits one batch.
    pub max_accepted: NonZeroUsize,
    /// Candidates judged per kernel batch, at most [`MAX_ELIGIBILITY_CANDIDATES`].
    pub batch_rows: NonZeroUsize,
}

/// One occurrence's lexical contribution: its raw FTS rank under the probe that ranked it best.
#[derive(Debug, Clone, PartialEq)]
pub struct Contribution {
    pub occurrence_id: String,
    pub class: OccurrenceClass,
    /// The engine's score for one probe, comparable only with other ranks of this request.
    pub rank: f64,
    /// Breaks equal-rank ties by selecting the lowest probe ordinal.
    pub ordinal: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteReason {
    /// A probe matched more rows than its scan bound, so lower-ranked hits were never seen.
    ScanBound,
    /// The accepted bound filled while unjudged or eligible candidates remained.
    AcceptedBound,
    BudgetExhausted,
    KernelIncarnationChanged,
    /// The kernel snapshot changed between eligibility batches, or the current classification generation was unknown.
    SnapshotChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// Every hit of every probe was seen and judged.
    Complete,
    /// Zero probes: no MATCH ran and there is nothing to rank.
    Empty,
    Incomplete(IncompleteReason),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Consumed {
    /// Probes the request compiled to, repeats included; a repeated probe is counted here without running the engine again.
    pub probes: usize,
    /// Live rows taken in comparator order, summed over `probes`; a repeated probe contributes its first run's count.
    pub scanned_rows: usize,
    pub judged: usize,
    pub batches: usize,
    /// Candidates the kernel judged ineligible before or at final revalidation, by verdict in judgment order.
    pub excluded: Vec<(EligibilityVerdict, usize)>,
}

impl Consumed {
    fn exclude(&mut self, verdict: EligibilityVerdict) {
        tally_exclusion(&mut self.excluded, verdict);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Retrieval {
    /// Accepted, re-judged contributions in comparator order; at most one per occurrence.
    pub contributions: Vec<Contribution>,
    pub completion: Completion,
    /// Records the kernel snapshot used to judge every contribution; `None` if no batch ran.
    pub snapshot: Option<EgressSnapshot>,
    pub incarnation: Option<CommitReadIncarnation>,
    pub consumed: Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RetrievalRefusal {
    #[error("the request compiles to {probes} probes, over the {bound} probe bound")]
    ProbesOverBound { probes: usize, bound: usize },
    #[error(
        "the {bound} bound of {value} exceeds the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate batch"
    )]
    BatchOverBound { bound: &'static str, value: usize },
    #[error("the request's budget ended before any probe completed")]
    BudgetExhausted,
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    AfterBatch(usize),
    BeforeRevalidation,
}

#[derive(Debug, Clone)]
struct Hit {
    candidate: OccurrenceCandidate,
    rank: f64,
    ordinal: usize,
}

fn better(challenger: &Hit, incumbent: &Hit) -> bool {
    challenger
        .rank
        .total_cmp(&incumbent.rank)
        .then_with(|| challenger.ordinal.cmp(&incumbent.ordinal))
        == Ordering::Less
}

fn comparator((left_id, left): &(String, Hit), (right_id, right): &(String, Hit)) -> Ordering {
    left.rank
        .total_cmp(&right.rank)
        .then_with(|| left_id.cmp(right_id))
}

/// The inner `LIMIT` bounds join and tombstone lookup work by `scan_rows`, not match-set size.
/// The `LEFT JOIN` keeps lexical rows with no occurrence, and the last column marks a missing or tombstoned occurrence dead,
/// so [`scan`] can tell an exhausted match set from a page that lost rows to the filter.
/// Under the [`super::index`] invariant that tombstoning deletes the lexical row, a dead row means an inconsistent projection.
const PROBE_SQL: &str = "SELECT h.occurrence_id, h.rank, o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            o.occurrence_id IS NOT NULL
            AND NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=h.occurrence_id)
     FROM (SELECT occurrence_id, rank FROM lexical WHERE lexical MATCH ?1
           ORDER BY rank, occurrence_id LIMIT ?2 OFFSET ?3) h
     LEFT JOIN occurrences o ON o.occurrence_id=h.occurrence_id
     ORDER BY h.rank, h.occurrence_id";

/// # Errors
///
/// A budget that ends before any probe completes is [`RetrievalRefusal::BudgetExhausted`]; one that ends later leaves the result [`Completion::Incomplete`] with no contributions.
/// A statement interrupted through the connection's progress handler ends the request the same way as an exhausted budget.
pub fn retrieve(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    probes: &[Probe],
    authority: Authority<'_>,
    bounds: RetrievalBounds,
    budget: &EvalBudget,
) -> Result<Retrieval, RetrievalRefusal> {
    retrieve_inner(conn, kernel, probes, authority, bounds, budget, |_| {})
}

/// `hook` runs after every admission batch and once before the final re-judgment, so a test can change the kernel or the budget in those windows.
#[cfg(feature = "test-support")]
pub fn retrieve_with_hook_for_test(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    probes: &[Probe],
    authority: Authority<'_>,
    bounds: RetrievalBounds,
    budget: &EvalBudget,
    hook: impl FnMut(Window),
) -> Result<Retrieval, RetrievalRefusal> {
    retrieve_inner(conn, kernel, probes, authority, bounds, budget, hook)
}

fn retrieve_inner(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    probes: &[Probe],
    authority: Authority<'_>,
    bounds: RetrievalBounds,
    budget: &EvalBudget,
    mut hook: impl FnMut(Window),
) -> Result<Retrieval, RetrievalRefusal> {
    budget
        .check()
        .map_err(|_| RetrievalRefusal::BudgetExhausted)?;
    for (bound, value) in [
        ("max_accepted", bounds.max_accepted.get()),
        ("batch_rows", bounds.batch_rows.get()),
    ] {
        if value > MAX_ELIGIBILITY_CANDIDATES {
            return Err(RetrievalRefusal::BatchOverBound { bound, value });
        }
    }
    if probes.len() > bounds.max_probes.get() {
        return Err(RetrievalRefusal::ProbesOverBound {
            probes: probes.len(),
            bound: bounds.max_probes.get(),
        });
    }
    let mut retrieval = Retrieval {
        contributions: Vec::new(),
        completion: if probes.is_empty() {
            Completion::Empty
        } else {
            Completion::Complete
        },
        snapshot: None,
        incarnation: None,
        consumed: Consumed::default(),
    };
    let mut best: BTreeMap<String, Hit> = BTreeMap::new();
    let mut outcomes: HashMap<&Probe, (usize, bool)> = HashMap::new();
    for (ordinal, probe) in probes.iter().enumerate() {
        if budget.is_exhausted() {
            return exhausted(retrieval);
        }
        let outcome = match outcomes.get(probe) {
            Some(&replayed) => Ok(replayed),
            None => scan(conn, probe, ordinal, bounds.scan_rows, budget, &mut best).inspect(
                |&outcome| {
                    outcomes.insert(probe, outcome);
                },
            ),
        };
        match outcome {
            Ok((rows, truncated)) => {
                retrieval.consumed.probes += 1;
                retrieval.consumed.scanned_rows += rows;
                if truncated {
                    incomplete(&mut retrieval, IncompleteReason::ScanBound);
                }
            }
            Err(ScanStop::Budget) => return exhausted(retrieval),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        }
    }
    let mut ordered: Vec<(String, Hit)> = best.into_iter().collect();
    ordered.sort_by(comparator);
    let accepted = admit(
        kernel,
        authority,
        bounds,
        budget,
        ordered,
        &mut retrieval,
        &mut hook,
    )?;
    hook(Window::BeforeRevalidation);
    revalidate(kernel, authority, budget, accepted, &mut retrieval)?;
    if budget.is_exhausted() {
        return exhausted(retrieval);
    }
    Ok(retrieval)
}

fn exhausted(mut retrieval: Retrieval) -> Result<Retrieval, RetrievalRefusal> {
    if retrieval.consumed.probes == 0 {
        return Err(RetrievalRefusal::BudgetExhausted);
    }
    retrieval.contributions.clear();
    incomplete(&mut retrieval, IncompleteReason::BudgetExhausted);
    Ok(retrieval)
}

fn incomplete(retrieval: &mut Retrieval, reason: IncompleteReason) {
    if reason == IncompleteReason::BudgetExhausted || retrieval.completion == Completion::Complete {
        retrieval.completion = Completion::Incomplete(reason);
    }
}

/// Reads the ordered match set one page of `scan_rows + 1` rows at a time and stops at the first live row past `scan_rows`,
/// which reports truncation without retaining that row.
/// A page that came back full but lost rows to the dead filter is followed by the next page; a short page ends the match set.
fn scan(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    scan_rows: NonZeroUsize,
    budget: &EvalBudget,
    best: &mut BTreeMap<String, Hit>,
) -> Result<(usize, bool), ScanStop> {
    let page = i64::try_from(scan_rows.get().saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(PROBE_SQL)?;
    let mut seen = 0;
    let mut offset: i64 = 0;
    loop {
        let mut rows = statement.query(rusqlite::params![probe, page, offset])?;
        let mut fetched: i64 = 0;
        while let Some(row) = rows.next()? {
            budget.check().map_err(|_| ScanStop::Budget)?;
            fetched += 1;
            if !row.get::<_, bool>(6)? {
                continue;
            }
            if seen == scan_rows.get() {
                return Ok((seen, true));
            }
            seen += 1;
            let (occurrence_id, hit) = hit(row, ordinal)?;
            if best
                .get(&occurrence_id)
                .is_none_or(|incumbent| better(&hit, incumbent))
            {
                best.insert(occurrence_id, hit);
            }
        }
        if fetched < page {
            return Ok((seen, false));
        }
        offset = offset.saturating_add(page);
    }
}

fn hit(row: &rusqlite::Row<'_>, ordinal: usize) -> Result<(String, Hit), ScanStop> {
    let occurrence_id: String = row.get(0)?;
    let class =
        OccurrenceClass::from_code(&row.get::<_, String>(2)?).ok_or(ProjectionError::CorruptRow)?;
    let hit = Hit {
        candidate: OccurrenceCandidate::new(
            occurrence_id.clone(),
            class,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ),
        rank: row.get(1)?,
        ordinal,
    };
    Ok((occurrence_id, hit))
}

/// Returns `SnapshotChanged` or `KernelIncarnationChanged` if kernel state differs from the first batch,
/// and `None` when the budget ended during the judgment, which is then already recorded on `retrieval`.
fn judge_batch(
    kernel: &KernelStore,
    authority: Authority<'_>,
    batch: &[(String, Hit)],
    budget: &EvalBudget,
    retrieval: &mut Retrieval,
) -> Result<Option<(EligibilityReport, Option<IncompleteReason>)>, RetrievalRefusal> {
    let candidates: Vec<OccurrenceCandidate> =
        batch.iter().map(|(_, hit)| hit.candidate.clone()).collect();
    let (report, moved) = match judge_tracked(
        kernel,
        authority,
        &candidates,
        budget,
        &mut retrieval.snapshot,
        &mut retrieval.incarnation,
    ) {
        Ok(judged) => judged,
        Err(KernelError::Deadline) => {
            incomplete(retrieval, IncompleteReason::BudgetExhausted);
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    retrieval.consumed.batches += 1;
    retrieval.consumed.judged += batch.len();
    let moved = moved.map(|moved| match moved {
        AuthorityMoved::Incarnation => IncompleteReason::KernelIncarnationChanged,
        AuthorityMoved::Snapshot => IncompleteReason::SnapshotChanged,
    });
    Ok(Some((report, moved)))
}

/// Judges candidates in comparator order, `batch_rows` at a time, and accepts eligible ones until `max_accepted` fills.
/// An ineligible leader consumes judgment work and no accepted slot, so eligible tails behind it are still reached.
fn admit(
    kernel: &KernelStore,
    authority: Authority<'_>,
    bounds: RetrievalBounds,
    budget: &EvalBudget,
    ordered: Vec<(String, Hit)>,
    retrieval: &mut Retrieval,
    hook: &mut impl FnMut(Window),
) -> Result<Vec<(String, Hit)>, RetrievalRefusal> {
    let mut accepted: Vec<(String, Hit)> = Vec::new();
    let mut rest = ordered.into_iter();
    loop {
        let batch: Vec<(String, Hit)> = rest.by_ref().take(bounds.batch_rows.get()).collect();
        if batch.is_empty() {
            break;
        }
        if accepted.len() == bounds.max_accepted.get() {
            incomplete(retrieval, IncompleteReason::AcceptedBound);
            break;
        }
        if budget.is_exhausted() {
            incomplete(retrieval, IncompleteReason::BudgetExhausted);
            break;
        }
        let Some((report, moved)) = judge_batch(kernel, authority, &batch, budget, retrieval)?
        else {
            break;
        };
        if let Some(reason) = moved {
            incomplete(retrieval, reason);
            // The moved batch's verdicts describe other facts, so none is accepted; its exclusions are still judged work.
            for judged in report.occurrences {
                if let Disposition::PolicyExcluded(verdict) = judged.disposition {
                    retrieval.consumed.exclude(verdict);
                }
            }
            break;
        }
        for ((occurrence_id, hit), judged) in batch.into_iter().zip(report.occurrences) {
            match judged.disposition {
                Disposition::Eligible if accepted.len() == bounds.max_accepted.get() => {
                    incomplete(retrieval, IncompleteReason::AcceptedBound);
                }
                Disposition::Eligible => accepted.push((occurrence_id, hit)),
                Disposition::PolicyExcluded(verdict) => retrieval.consumed.exclude(verdict),
            }
        }
        hook(Window::AfterBatch(retrieval.consumed.batches));
    }
    Ok(accepted)
}

/// Re-judges every accepted occurrence in one batch and keeps only those the kernel still admits, so a canonical change after admission cannot reach the caller.
fn revalidate(
    kernel: &KernelStore,
    authority: Authority<'_>,
    budget: &EvalBudget,
    accepted: Vec<(String, Hit)>,
    retrieval: &mut Retrieval,
) -> Result<(), RetrievalRefusal> {
    if accepted.is_empty() {
        return Ok(());
    }
    if budget.is_exhausted() {
        incomplete(retrieval, IncompleteReason::BudgetExhausted);
        return Ok(());
    }
    let Some((report, moved)) = judge_batch(kernel, authority, &accepted, budget, retrieval)?
    else {
        return Ok(());
    };
    if let Some(reason) = moved {
        incomplete(retrieval, reason);
    }
    retrieval.snapshot = Some(report.snapshot);
    retrieval.incarnation = Some(report.incarnation);
    for ((occurrence_id, hit), judged) in accepted.into_iter().zip(report.occurrences) {
        match judged.disposition {
            Disposition::Eligible => retrieval.contributions.push(Contribution {
                occurrence_id,
                class: hit.candidate.class,
                rank: hit.rank,
                ordinal: hit.ordinal,
            }),
            Disposition::PolicyExcluded(verdict) => retrieval.consumed.exclude(verdict),
        }
    }
    Ok(())
}
