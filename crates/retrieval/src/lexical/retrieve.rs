//! Each occurrence keeps its best probe; canonical eligibility is judged in bounded batches before acceptance; accepted contributions are re-judged before return.
//! This module does not read payload bytes; each contribution contains an occurrence identifier and raw FTS rank.
//!
//! [`scan`] needs only the projection connection and [`admit`] needs only the kernel, so a caller
//! can release the projection connection before any kernel reader is taken; [`retrieve`] runs both
//! under one connection for callers that hold nothing else.
//!
//! Ordering is one comparator throughout: lower raw rank first, then occurrence identifier bytes ascending.
//! An occurrence hit by several probes keeps the lowest rank, and among equal ranks the lowest probe ordinal, so duplicate or permuted probes leave the ranking unchanged.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CommitReadIncarnation, EgressSnapshot, EligibilityCandidate, EligibilityVerdict, KernelError,
    KernelStore, MAX_ELIGIBILITY_CANDIDATES,
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
    /// The terms the kernel judged, kept so a later revalidation judges the same facts without another projection read.
    pub candidate: EligibilityCandidate,
}

impl Contribution {
    pub fn occurrence_candidate(&self) -> OccurrenceCandidate {
        OccurrenceCandidate {
            occurrence_id: self.occurrence_id.clone(),
            class: self.class,
            candidate: self.candidate.clone(),
        }
    }
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
    pub probes: usize,
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

/// Filters orphaned and tombstoned matches before `LIMIT`.
const PROBE_SQL: &str = "SELECT l.occurrence_id, l.rank, o.class, o.source_object_id, o.revision, o.source_artifact_digest, 0
     FROM lexical l JOIN occurrences o ON o.occurrence_id=l.occurrence_id
     WHERE lexical MATCH ?1
       AND NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id)
     ORDER BY l.rank, l.occurrence_id
     LIMIT ?2";

/// The last column marks a shortlisted row that is orphaned or tombstoned in the projection.
/// Such a row holds a slot a live row would otherwise take, so the result is exact only when no row is marked.
const LATE_JOIN_SQL: &str = "SELECT l.occurrence_id, l.rank, o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            o.occurrence_id IS NULL OR EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=l.occurrence_id)
     FROM (SELECT occurrence_id, rank FROM lexical WHERE lexical MATCH ?1
           ORDER BY rank, occurrence_id LIMIT ?2) l
     LEFT JOIN occurrences o ON o.occurrence_id=l.occurrence_id
     ORDER BY l.rank, l.occurrence_id";

/// Probe hits read from the projection in comparator order, not yet judged by the kernel.
///
/// Retains its `RetrievalBounds` so admission uses the bounds that produced its hits.
#[derive(Debug, Clone)]
pub struct Scan {
    ordered: Vec<(String, Hit)>,
    retrieval: Retrieval,
    bounds: RetrievalBounds,
}

impl Scan {
    /// Distinct occurrences the probes hit, before any kernel verdict.
    pub fn hits(&self) -> usize {
        self.ordered.len()
    }
}

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
    let scanned = scan(conn, probes, bounds, budget)?;
    admit_inner(kernel, authority, scanned, budget, |_| {})
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
    let scanned = scan(conn, probes, bounds, budget)?;
    admit_inner(kernel, authority, scanned, budget, hook)
}

/// Runs every probe against the projection and keeps each occurrence's best hit.
///
/// # Errors
///
/// Refuses bounds the kernel's eligibility batch could never serve, more probes than `max_probes`, and a budget that ends before any probe completes; a budget that ends later yields a [`Scan`] whose admission is already [`Completion::Incomplete`] with no hits.
pub fn scan(
    conn: &GuardedConn<'_>,
    probes: &[Probe],
    bounds: RetrievalBounds,
    budget: &EvalBudget,
) -> Result<Scan, RetrievalRefusal> {
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
    for (ordinal, probe) in probes.iter().enumerate() {
        if budget.is_exhausted() {
            return exhausted_scan(retrieval, bounds);
        }
        match scan_probe(conn, probe, ordinal, bounds.scan_rows, budget, &mut best) {
            Ok((rows, truncated)) => {
                retrieval.consumed.probes += 1;
                retrieval.consumed.scanned_rows += rows;
                if truncated {
                    incomplete(&mut retrieval, IncompleteReason::ScanBound);
                }
            }
            Err(ScanStop::Budget) => return exhausted_scan(retrieval, bounds),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        }
    }
    let mut ordered: Vec<(String, Hit)> = best.into_iter().collect();
    ordered.sort_by(comparator);
    Ok(Scan {
        ordered,
        retrieval,
        bounds,
    })
}

/// Judges the scan's hits in bounded kernel batches, accepts eligible ones until `max_accepted` fills, and re-judges the accepted set once before returning.
///
/// # Errors
///
/// Returns kernel errors except [`KernelError::Deadline`], which leaves the result [`Completion::Incomplete`] with no contributions.
pub fn admit(
    kernel: &KernelStore,
    authority: Authority<'_>,
    scanned: Scan,
    budget: &EvalBudget,
) -> Result<Retrieval, RetrievalRefusal> {
    admit_inner(kernel, authority, scanned, budget, |_| {})
}

fn admit_inner(
    kernel: &KernelStore,
    authority: Authority<'_>,
    scanned: Scan,
    budget: &EvalBudget,
    mut hook: impl FnMut(Window),
) -> Result<Retrieval, RetrievalRefusal> {
    let Scan {
        ordered,
        mut retrieval,
        bounds,
    } = scanned;
    if retrieval.completion == Completion::Incomplete(IncompleteReason::BudgetExhausted) {
        return Ok(retrieval);
    }
    let accepted = admit_batches(
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

/// A budget-exhausted scan retains no hits; admission returns its recorded completion unchanged.
fn exhausted_scan(retrieval: Retrieval, bounds: RetrievalBounds) -> Result<Scan, RetrievalRefusal> {
    Ok(Scan {
        ordered: Vec::new(),
        retrieval: exhausted(retrieval)?,
        bounds,
    })
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

/// The probe query reads one row past `scan_rows` to report truncation without retaining the extra row.
///
/// A stale shortlisted row can underfill the result, so rerun `PROBE_SQL`.
/// Live rows already recorded in `best` are a prefix of `PROBE_SQL`'s rows.
fn scan_probe(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    scan_rows: NonZeroUsize,
    budget: &EvalBudget,
    best: &mut BTreeMap<String, Hit>,
) -> Result<(usize, bool), ScanStop> {
    if let Some(result) = scan_with(conn, LATE_JOIN_SQL, probe, ordinal, scan_rows, budget, best)? {
        return Ok(result);
    }
    scan_with(conn, PROBE_SQL, probe, ordinal, scan_rows, budget, best)?
        .ok_or(ScanStop::Projection(ProjectionError::CorruptRow))
}

/// `None` when a shortlisted row is stale and the result may be inexact.
fn scan_with(
    conn: &GuardedConn<'_>,
    sql: &str,
    probe: &Probe,
    ordinal: usize,
    scan_rows: NonZeroUsize,
    budget: &EvalBudget,
    best: &mut BTreeMap<String, Hit>,
) -> Result<Option<(usize, bool)>, ScanStop> {
    let limit = i64::try_from(scan_rows.get().saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(sql)?;
    let mut rows = statement.query(rusqlite::params![probe, limit])?;
    let mut seen = 0;
    while let Some(row) = rows.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        if row.get::<_, bool>(6)? {
            return Ok(None);
        }
        if seen == scan_rows.get() {
            return Ok(Some((seen, true)));
        }
        seen += 1;
        let occurrence_id: String = row.get(0)?;
        let class = OccurrenceClass::from_code(&row.get::<_, String>(2)?)
            .ok_or(ProjectionError::CorruptRow)?;
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
        if best
            .get(&occurrence_id)
            .is_none_or(|incumbent| better(&hit, incumbent))
        {
            best.insert(occurrence_id, hit);
        }
    }
    Ok(Some((seen, false)))
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
fn admit_batches(
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
                candidate: hit.candidate.candidate,
            }),
            Disposition::PolicyExcluded(verdict) => retrieval.consumed.exclude(verdict),
        }
    }
    Ok(())
}
