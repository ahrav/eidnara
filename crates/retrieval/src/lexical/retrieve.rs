//! Each occurrence keeps its best probe; canonical eligibility is judged in bounded batches before acceptance; accepted contributions are re-judged before return.
//! This module does not read payload bytes; each contribution contains an occurrence identifier and raw FTS rank.
//!
//! [`scan`] needs only the projection connection and [`admit`] needs only the kernel, so a caller
//! can release the projection connection before any kernel reader is taken; [`retrieve`] runs both
//! under one connection for callers that hold nothing else.
//!
//! Ordering is one comparator throughout: lower raw rank first, then occurrence identifier bytes ascending.
//! An occurrence hit by several probes keeps the lowest rank, and among equal ranks the lowest probe ordinal, so duplicate or permuted probes leave the ranking unchanged.
//! A probe repeated later in the request therefore cannot change the ranking, and the engine counts and ranks it only once.
//!
//! Ranking work is bounded before the engine ranks anything. Each distinct probe is first counted in rowid order up to
//! [`RetrievalBounds::qualifying_matches`] plus one lookahead row. Probes with at most
//! [`RetrievalBounds::qualifying_matches`] matches qualify and are ranked in increasing count order, original probe
//! order breaking ties, while the counts of the ranked probes sum to at most [`RetrievalBounds::rank_budget`]. When no
//! probe qualifies, the common probes are read in descending rowid order under the scan bound instead. A skipped probe
//! of either kind leaves the result incomplete.

use rusqlite::StatementStatus;
use std::cmp::Ordering;
use std::collections::HashSet;
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
    /// A probe matching at most this many rows qualifies to be ranked; counting reads one lookahead row past it.
    pub qualifying_matches: NonZeroUsize,
    /// The counts of the probes one request ranks sum to at most this many matches.
    pub rank_budget: NonZeroUsize,
}

/// One occurrence's lexical contribution: its raw FTS rank under the probe that ranked it best.
#[derive(Debug, Clone, PartialEq)]
pub struct Contribution {
    pub occurrence_id: String,
    pub class: OccurrenceClass,
    /// The engine's score for one probe, comparable only with other ranks of this request; a row read for an only-common query is not scored and carries `0.0`.
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
    /// A probe matched more than [`RetrievalBounds::qualifying_matches`] rows. When every matching probe is over that limit, retrieval reads each one's rows in descending rowid order and scores each row `0.0`.
    /// Lexical rowids derive from occurrence identifier words, so this order follows identifier placement rather than occurrence age.
    CommonTerms,
    /// A qualifying probe's count would take the ranked matches past [`RetrievalBounds::rank_budget`], so the engine skipped it.
    RankBudget,
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
    /// Live rows kept within the scan bound, summed over the distinct probes that ran; a repeated probe adds nothing.
    pub scanned_rows: usize,
    /// Rows read to count the distinct probes, lookahead rows included.
    pub counted_rows: usize,
    /// Matches the engine ranked: the summed counts of the ranked probes, at most [`RetrievalBounds::rank_budget`].
    pub ranked_matches: usize,
    pub judged: usize,
    pub batches: usize,
    /// SQLite virtual-machine operations (`SQLITE_STMTSTATUS_VM_STEP`) the count and scan statements executed for the probes that finished; the count grows with the rows those statements visit. A ranked probe's FTS5 rank sort runs in a nested statement SQLite keeps internal, so its scoring is observed through `ranked_matches`.
    pub sql_steps: u64,
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
    /// Every reason recorded for an incomplete result, each once, in the order first recorded; `completion` names the one of highest precedence.
    pub reasons: Vec<IncompleteReason>,
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

impl Hit {
    fn id(&self) -> &str {
        &self.candidate.occurrence_id
    }
}

/// Sorting with `best_first` lets `dedup_by` retain each occurrence's lowest-rank hit, breaking ties by probe ordinal.
fn best_first(left: &Hit, right: &Hit) -> Ordering {
    left.id()
        .cmp(right.id())
        .then_with(|| left.rank.total_cmp(&right.rank))
        .then_with(|| left.ordinal.cmp(&right.ordinal))
}

fn comparator(left: &Hit, right: &Hit) -> Ordering {
    left.rank
        .total_cmp(&right.rank)
        .then_with(|| left.id().cmp(right.id()))
}

/// Counts a probe's matches in the engine's rowid order inside SQLite, computing no rank; `?2` is the count limit.
const COUNT_SQL: &str = "SELECT count(*) FROM (SELECT rowid FROM lexical WHERE lexical MATCH ?1 ORDER BY rowid LIMIT ?2)";

/// `?2` is the probe's match count from [`count_probe`].
const RANKED_SQL: &str =
    "SELECT rank, occurrence_id FROM lexical WHERE lexical MATCH ?1 ORDER BY rank LIMIT ?2";

const COMMON_SQL: &str = "SELECT rowid, occurrence_id FROM lexical WHERE lexical MATCH ?1 AND rowid <= ?2 ORDER BY rowid DESC LIMIT ?3";

/// A missing occurrence returns no row and a tombstoned one returns `false` in the last column, so the reader can tell an
/// exhausted match set from rows lost to the filter.
/// Under the [`super::index`] invariant that tombstoning deletes the lexical row, a dead row means an inconsistent projection.
const ROW_SQL: &str = "SELECT o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=?1)
     FROM occurrences o
     WHERE o.occurrence_id=?1";

/// The raw score a common probe's rows carry: they are read in rowid order, not ranked.
const UNRANKED: f64 = 0.0;

/// Probe hits read from the projection in comparator order, not yet judged by the kernel.
///
/// Retains its `RetrievalBounds` so admission uses the bounds that produced its hits.
#[derive(Debug, Clone)]
pub struct Scan {
    ordered: Vec<Hit>,
    retrieval: Retrieval,
    bounds: RetrievalBounds,
}

impl Scan {
    /// Distinct occurrences the probes hit, before any kernel verdict.
    pub fn hits(&self) -> usize {
        self.ordered.len()
    }

    /// The hit occurrences in comparator order, before any kernel verdict.
    pub fn hit_ids(&self) -> impl Iterator<Item = &str> {
        self.ordered.iter().map(Hit::id)
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
        reasons: Vec::new(),
    };
    let mut hits: Vec<Hit> = Vec::new();
    // Each distinct probe is counted once, under its first ordinal.
    let mut counted: HashSet<&Probe> = HashSet::new();
    let mut distinct: Vec<(usize, &Probe, usize)> = Vec::new();
    for (ordinal, probe) in probes.iter().enumerate() {
        if budget.is_exhausted() {
            return exhausted_scan(retrieval, bounds);
        }
        if !counted.contains(probe) {
            match count_probe(conn, probe, bounds.qualifying_matches, budget) {
                Ok((count, steps)) => {
                    retrieval.consumed.counted_rows += count;
                    retrieval.consumed.sql_steps += steps;
                    counted.insert(probe);
                    distinct.push((ordinal, probe, count));
                }
                Err(ScanStop::Budget) => return exhausted_scan(retrieval, bounds),
                Err(ScanStop::Projection(error)) => return Err(error.into()),
            }
        }
        retrieval.consumed.probes += 1;
    }
    // A probe with no match contributes nothing, so it neither qualifies nor counts as common.
    let (mut qualifying, common): (Vec<_>, Vec<_>) = distinct
        .into_iter()
        .filter(|&(_, _, count)| count > 0)
        .partition(|&(_, _, count)| count <= bounds.qualifying_matches.get());
    qualifying.sort_by_key(|&(ordinal, _, count)| (count, ordinal));
    if !common.is_empty() {
        incomplete(&mut retrieval, IncompleteReason::CommonTerms);
    }
    let runs = if qualifying.is_empty() {
        common
            .into_iter()
            .map(|(ordinal, probe, _)| (ordinal, probe, Run::Common))
            .collect::<Vec<_>>()
    } else {
        let mut ranked = Vec::new();
        for (ordinal, probe, count) in qualifying {
            if retrieval.consumed.ranked_matches + count > bounds.rank_budget.get() {
                incomplete(&mut retrieval, IncompleteReason::RankBudget);
                break;
            }
            retrieval.consumed.ranked_matches += count;
            ranked.push((ordinal, probe, Run::Ranked(count)));
        }
        ranked
    };
    for (ordinal, probe, run) in runs {
        if budget.is_exhausted() {
            return exhausted_scan(retrieval, bounds);
        }
        match scan_probe(
            conn,
            probe,
            ordinal,
            run,
            bounds.scan_rows,
            budget,
            &mut hits,
        ) {
            Ok((rows, truncated, steps)) => {
                retrieval.consumed.scanned_rows += rows;
                retrieval.consumed.sql_steps += steps;
                if truncated {
                    incomplete(&mut retrieval, IncompleteReason::ScanBound);
                }
            }
            Err(ScanStop::Budget) => return exhausted_scan(retrieval, bounds),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        }
    }
    hits.sort_by(best_first);
    hits.dedup_by(|later, kept| later.id() == kept.id());
    hits.sort_by(comparator);
    Ok(Scan {
        ordered: hits,
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
    if !retrieval.reasons.contains(&reason) {
        retrieval.reasons.push(reason);
    }
    let decides = match retrieval.completion {
        Completion::Complete => true,
        Completion::Empty => reason == IncompleteReason::BudgetExhausted,
        Completion::Incomplete(current) => precedence(reason) > precedence(current),
    };
    if decides {
        retrieval.completion = Completion::Incomplete(reason);
    }
}

/// Equal-precedence reasons keep the first recorded one as the completion.
fn precedence(reason: IncompleteReason) -> u8 {
    match reason {
        IncompleteReason::BudgetExhausted => 2,
        IncompleteReason::KernelIncarnationChanged | IncompleteReason::SnapshotChanged => 1,
        IncompleteReason::ScanBound
        | IncompleteReason::CommonTerms
        | IncompleteReason::RankBudget
        | IncompleteReason::AcceptedBound => 0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    /// Ranked by the engine; the probe's counted matches.
    Ranked(usize),
    Common,
}

/// The probe's match count in rowid order, stopping one row past `qualifying` so an overflow is told apart from exactly the threshold without a full count.
fn count_probe(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    qualifying: NonZeroUsize,
    budget: &EvalBudget,
) -> Result<(usize, u64), ScanStop> {
    budget.check().map_err(|_| ScanStop::Budget)?;
    let limit = i64::try_from(qualifying.get().saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(COUNT_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let count: i64 = statement.query_row(rusqlite::params![probe, limit], |row| row.get(0))?;
    Ok((
        usize::try_from(count).unwrap_or(usize::MAX),
        vm_steps(&statement),
    ))
}

/// The VM steps `statement` ran since its counter was last reset.
fn vm_steps(statement: &rusqlite::Statement<'_>) -> u64 {
    // SQLite keeps the counter unsigned and hands it back as an `int`.
    u64::from(
        statement
            .get_status(StatementStatus::VmStep)
            .cast_unsigned(),
    )
}

/// The probe scan appends its best `scan_rows` live rows to `hits`; the returned boolean is `true` when additional live rows exist past the bound.
/// A ranked run reads the engine's rank order and stops after the group of rows equal in rank to the last row within the bound, so the
/// occurrence-identifier tie-break, not storage order, decides which equal-rank rows the bound keeps.
fn scan_probe(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    run: Run,
    scan_rows: NonZeroUsize,
    budget: &EvalBudget,
    hits: &mut Vec<Hit>,
) -> Result<(usize, bool, u64), ScanStop> {
    let bound = scan_rows.get();
    let mut taken: Vec<Hit> = Vec::new();
    let mut detail = conn.prepare_cached(ROW_SQL)?;
    detail.reset_status(StatementStatus::VmStep);
    let mut steps = 0;
    let truncated = match run {
        Run::Common => {
            let (truncated, common) =
                scan_common(conn, &mut detail, probe, ordinal, bound, budget, &mut taken)?;
            steps += common;
            truncated
        }
        Run::Ranked(count) => {
            let mut statement = conn.prepare_cached(RANKED_SQL)?;
            statement.reset_status(StatementStatus::VmStep);
            // The engine's rank sorter scores and orders every counted match once; rows are stepped out only until the bound settles.
            let ranked = statement.query_map(
                rusqlite::params![probe, i64::try_from(count).unwrap_or(i64::MAX)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let truncated = settle_ranked(ranked, &mut detail, budget, ordinal, bound, &mut taken)?;
            steps += vm_steps(&statement);
            truncated
        }
    };
    steps += vm_steps(&detail);
    let seen = taken.len();
    hits.append(&mut taken);
    Ok((seen, truncated, steps))
}

/// `scan_common` takes a common probe's first `bound` live rows in descending rowid order and returns `true` when another live row exists, with the VM steps its page statement ran.
/// Each page reads `bound + 1` rows below the last rowid seen, so a dead row costs one read and no kept slot, and a short page ends the match set.
fn scan_common(
    conn: &GuardedConn<'_>,
    detail: &mut storage::CachedStatement<'_>,
    probe: &Probe,
    ordinal: usize,
    bound: usize,
    budget: &EvalBudget,
    taken: &mut Vec<Hit>,
) -> Result<(bool, u64), ScanStop> {
    let mut statement = conn.prepare_cached(COMMON_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let page = i64::try_from(bound.saturating_add(1)).unwrap_or(i64::MAX);
    let mut ceiling = i64::MAX;
    let truncated = 'pages: loop {
        let mut rows = statement.query(rusqlite::params![probe, ceiling, page])?;
        let mut fetched: i64 = 0;
        while let Some(row) = rows.next()? {
            budget.check().map_err(|_| ScanStop::Budget)?;
            fetched += 1;
            let rowid: i64 = row.get(0)?;
            ceiling = rowid.saturating_sub(1);
            if let Some(found) = live_row(detail, row.get(1)?, UNRANKED, ordinal)? {
                if taken.len() == bound {
                    break 'pages true;
                }
                taken.push(found);
            }
        }
        if fetched < page {
            break false;
        }
    };
    Ok((truncated, vm_steps(&statement)))
}

/// Keeps the best `bound` live rows of `ranked`, which is in the engine's rank order; `true` when a live row lies past the bound.
fn settle_ranked(
    ranked: impl Iterator<Item = rusqlite::Result<(f64, String)>>,
    detail: &mut storage::CachedStatement<'_>,
    budget: &EvalBudget,
    ordinal: usize,
    bound: usize,
    taken: &mut Vec<Hit>,
) -> Result<bool, ScanStop> {
    let mut group = Group {
        occurrence_ids: Vec::new(),
        rank: 0.0,
        ordinal,
        bound,
    };
    for row in ranked {
        let (rank, occurrence_id) = row?;
        budget.check().map_err(|_| ScanStop::Budget)?;
        if !group.occurrence_ids.is_empty()
            && rank.total_cmp(&group.rank) != Ordering::Equal
            && group.resolve(detail, taken)?
        {
            return Ok(true);
        }
        if taken.len() == bound {
            // Every kept row ranks strictly better than this one, so it only shows whether a live row lies past the bound.
            if live_row(detail, occurrence_id, rank, ordinal)?.is_some() {
                return Ok(true);
            }
            continue;
        }
        group.rank = rank;
        group.occurrence_ids.push(occurrence_id);
    }
    if group.occurrence_ids.is_empty() {
        return Ok(false);
    }
    group.resolve(detail, taken)
}

/// The live occurrence behind one lexical row, or `None` for a dead row.
fn live_row(
    detail: &mut storage::CachedStatement<'_>,
    occurrence_id: String,
    rank: f64,
    ordinal: usize,
) -> Result<Option<Hit>, ScanStop> {
    let mut found = detail.query(rusqlite::params![occurrence_id])?;
    let Some(found) = found.next()? else {
        return Ok(None);
    };
    if !found.get::<_, bool>(4)? {
        return Ok(None);
    }
    hit(found, occurrence_id, rank, ordinal).map(Some)
}

/// Rows of one rank, in arrival order, waiting to be kept or cut at the scan bound.
struct Group {
    occurrence_ids: Vec<String>,
    rank: f64,
    ordinal: usize,
    bound: usize,
}

impl Group {
    /// Moves the group's live rows into `taken` up to the bound; `true` when a live row of the group lies past it.
    /// A group that crosses the bound is ordered by occurrence identifier first, so storage order never chooses which equal-rank rows stay.
    fn resolve(
        &mut self,
        detail: &mut storage::CachedStatement<'_>,
        taken: &mut Vec<Hit>,
    ) -> Result<bool, ScanStop> {
        if taken.len() + self.occurrence_ids.len() > self.bound {
            self.occurrence_ids.sort_unstable();
        }
        for occurrence_id in self.occurrence_ids.drain(..) {
            if let Some(found) = live_row(detail, occurrence_id, self.rank, self.ordinal)? {
                if taken.len() == self.bound {
                    return Ok(true);
                }
                taken.push(found);
            }
        }
        Ok(false)
    }
}

fn hit(
    row: &rusqlite::Row<'_>,
    occurrence_id: String,
    rank: f64,
    ordinal: usize,
) -> Result<Hit, ScanStop> {
    let class = row
        .get_ref(0)?
        .as_str()
        .ok()
        .and_then(OccurrenceClass::from_code)
        .ok_or(ProjectionError::CorruptRow)?;
    Ok(Hit {
        candidate: OccurrenceCandidate::new(
            occurrence_id,
            class,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
        ),
        rank,
        ordinal,
    })
}

/// Returns `SnapshotChanged` or `KernelIncarnationChanged` if kernel state differs from the first batch,
/// and `None` when the budget ended during the judgment, which is then already recorded on `retrieval`.
fn judge_batch(
    kernel: &KernelStore,
    authority: Authority<'_>,
    batch: &[Hit],
    budget: &EvalBudget,
    retrieval: &mut Retrieval,
) -> Result<Option<(EligibilityReport, Option<IncompleteReason>)>, RetrievalRefusal> {
    let candidates: Vec<OccurrenceCandidate> =
        batch.iter().map(|hit| hit.candidate.clone()).collect();
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
    ordered: Vec<Hit>,
    retrieval: &mut Retrieval,
    hook: &mut impl FnMut(Window),
) -> Result<Vec<Hit>, RetrievalRefusal> {
    let mut accepted: Vec<Hit> = Vec::new();
    let mut rest = ordered.into_iter();
    loop {
        let batch: Vec<Hit> = rest.by_ref().take(bounds.batch_rows.get()).collect();
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
        for (hit, judged) in batch.into_iter().zip(report.occurrences) {
            match judged.disposition {
                Disposition::Eligible if accepted.len() == bounds.max_accepted.get() => {
                    incomplete(retrieval, IncompleteReason::AcceptedBound);
                }
                Disposition::Eligible => accepted.push(hit),
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
    accepted: Vec<Hit>,
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
    for (hit, judged) in accepted.into_iter().zip(report.occurrences) {
        match judged.disposition {
            Disposition::Eligible => retrieval.contributions.push(Contribution {
                occurrence_id: hit.candidate.occurrence_id,
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
