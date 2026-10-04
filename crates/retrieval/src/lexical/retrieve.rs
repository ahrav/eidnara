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

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
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

/// Counts a probe's matches in the engine's rowid order inside SQLite, computing no rank; `?2` is the count limit.
const COUNT_SQL: &str = "SELECT count(*) FROM (SELECT rowid FROM lexical WHERE lexical MATCH ?1 ORDER BY rowid LIMIT ?2)";

/// A qualifying probe's rowids and ranks in the engine's rank order; `?2` is the probe's count, so the engine ranks no more rows than it counted.
const RANKED_SQL: &str =
    "SELECT rowid, rank FROM lexical WHERE lexical MATCH ?1 ORDER BY rank LIMIT ?2";

/// One page of a common probe's rowids in descending order, each at most `?2`, with at most `?3` rows; the engine computes no rank for them.
const COMMON_SQL: &str =
    "SELECT rowid FROM lexical WHERE lexical MATCH ?1 AND rowid <= ?2 ORDER BY rowid DESC LIMIT ?3";

/// One lexical row's occurrence. The `LEFT JOIN` keeps a lexical row with no occurrence, and the last column marks a missing or
/// tombstoned occurrence dead, so the reader can tell an exhausted match set from rows lost to the filter.
/// Under the [`super::index`] invariant that tombstoning deletes the lexical row, a dead row means an inconsistent projection.
const ROW_SQL: &str = "SELECT l.occurrence_id, o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            o.occurrence_id IS NOT NULL
            AND NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=l.occurrence_id)
     FROM lexical l LEFT JOIN occurrences o ON o.occurrence_id=l.occurrence_id
     WHERE l.rowid=?1";

/// One lexical row's occurrence identifier, read only for the rows of an equal-rank group that crosses the scan bound.
const ID_SQL: &str = "SELECT occurrence_id FROM lexical WHERE rowid=?1";

/// The raw score a common probe's rows carry: they are read in rowid order, not ranked.
const UNRANKED: f64 = 0.0;

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

    /// The hit occurrences in comparator order, before any kernel verdict.
    pub fn hit_ids(&self) -> impl Iterator<Item = &str> {
        self.ordered.iter().map(|(id, _)| id.as_str())
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
    let mut best: BTreeMap<String, Hit> = BTreeMap::new();
    // Each distinct probe is counted once, under its first ordinal.
    let mut counted: HashSet<&Probe> = HashSet::new();
    let mut distinct: Vec<(usize, &Probe, usize)> = Vec::new();
    for (ordinal, probe) in probes.iter().enumerate() {
        if budget.is_exhausted() {
            return exhausted_scan(retrieval, bounds);
        }
        if !counted.contains(probe) {
            match count_probe(conn, probe, bounds.qualifying_matches, budget) {
                Ok(count) => {
                    retrieval.consumed.counted_rows += count;
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
            &mut best,
        ) {
            Ok((rows, truncated)) => {
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
) -> Result<usize, ScanStop> {
    budget.check().map_err(|_| ScanStop::Budget)?;
    let limit = i64::try_from(qualifying.get().saturating_add(1)).unwrap_or(i64::MAX);
    let count: i64 = conn
        .prepare_cached(COUNT_SQL)?
        .query_row(rusqlite::params![probe, limit], |row| row.get(0))?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// Takes the probe's best `scan_rows` live rows in comparator order and merges each into `best`; `true` when live rows past the bound existed.
/// A ranked run reads the engine's rank order and stops after the group of rows equal in rank to the last row within the bound, so the
/// occurrence-identifier tie-break, not storage order, decides which equal-rank rows the bound keeps.
fn scan_probe(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    run: Run,
    scan_rows: NonZeroUsize,
    budget: &EvalBudget,
    best: &mut BTreeMap<String, Hit>,
) -> Result<(usize, bool), ScanStop> {
    let bound = scan_rows.get();
    let mut taken: Vec<(String, Hit)> = Vec::new();
    let mut detail = conn.prepare_cached(ROW_SQL)?;
    let truncated = match run {
        Run::Common => scan_common(conn, &mut detail, probe, ordinal, bound, budget, &mut taken)?,
        Run::Ranked(count) => {
            let mut statement = conn.prepare_cached(RANKED_SQL)?;
            let mut ids = conn.prepare_cached(ID_SQL)?;
            // The engine's rank sorter scores and orders every counted match once; rows are stepped out only until the bound settles.
            let ranked = statement.query_map(
                rusqlite::params![probe, i64::try_from(count).unwrap_or(i64::MAX)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            settle_ranked(
                ranked,
                &mut detail,
                &mut ids,
                budget,
                ordinal,
                bound,
                &mut taken,
            )?
        }
    };
    let seen = taken.len();
    for (occurrence_id, hit) in taken {
        if best
            .get(&occurrence_id)
            .is_none_or(|incumbent| better(&hit, incumbent))
        {
            best.insert(occurrence_id, hit);
        }
    }
    Ok((seen, truncated))
}

/// `scan_common` takes a common probe's first `bound` live rows in descending rowid order and returns `true` when another live row exists.
/// Each page reads `bound + 1` rows below the last rowid seen, so a dead row costs one read and no kept slot, and a short page ends the match set.
fn scan_common(
    conn: &GuardedConn<'_>,
    detail: &mut storage::CachedStatement<'_>,
    probe: &Probe,
    ordinal: usize,
    bound: usize,
    budget: &EvalBudget,
    taken: &mut Vec<(String, Hit)>,
) -> Result<bool, ScanStop> {
    let mut statement = conn.prepare_cached(COMMON_SQL)?;
    let page = i64::try_from(bound.saturating_add(1)).unwrap_or(i64::MAX);
    let mut ceiling = i64::MAX;
    loop {
        let mut rows = statement.query(rusqlite::params![probe, ceiling, page])?;
        let mut fetched: i64 = 0;
        while let Some(row) = rows.next()? {
            budget.check().map_err(|_| ScanStop::Budget)?;
            fetched += 1;
            let rowid: i64 = row.get(0)?;
            ceiling = rowid.saturating_sub(1);
            if let Some(found) = live_row(detail, rowid, UNRANKED, ordinal)? {
                if taken.len() == bound {
                    return Ok(true);
                }
                taken.push(found);
            }
        }
        if fetched < page {
            return Ok(false);
        }
    }
}

/// Keeps the best `bound` live rows of `ranked`, which is in the engine's rank order; `true` when a live row lies past the bound.
fn settle_ranked(
    ranked: impl Iterator<Item = rusqlite::Result<(i64, f64)>>,
    detail: &mut storage::CachedStatement<'_>,
    ids: &mut storage::CachedStatement<'_>,
    budget: &EvalBudget,
    ordinal: usize,
    bound: usize,
    taken: &mut Vec<(String, Hit)>,
) -> Result<bool, ScanStop> {
    let mut group = Group {
        rowids: Vec::new(),
        rank: 0.0,
        ordinal,
        bound,
    };
    for row in ranked {
        let (rowid, rank) = row?;
        budget.check().map_err(|_| ScanStop::Budget)?;
        if !group.rowids.is_empty()
            && rank.total_cmp(&group.rank) != Ordering::Equal
            && group.resolve(detail, ids, budget, taken)?
        {
            return Ok(true);
        }
        if taken.len() == bound {
            // Every kept row ranks strictly better than this one, so it only shows whether a live row lies past the bound.
            if live_row(detail, rowid, rank, ordinal)?.is_some() {
                return Ok(true);
            }
            continue;
        }
        group.rank = rank;
        group.rowids.push(rowid);
    }
    if group.rowids.is_empty() {
        return Ok(false);
    }
    group.resolve(detail, ids, budget, taken)
}

/// The live occurrence behind one lexical row, or `None` for a dead row.
fn live_row(
    detail: &mut storage::CachedStatement<'_>,
    rowid: i64,
    rank: f64,
    ordinal: usize,
) -> Result<Option<(String, Hit)>, ScanStop> {
    let mut found = detail.query(rusqlite::params![rowid])?;
    let Some(found) = found.next()? else {
        return Err(ProjectionError::CorruptRow.into());
    };
    if !found.get::<_, bool>(5)? {
        return Ok(None);
    }
    hit(found, rank, ordinal).map(Some)
}

/// Rows of one rank, in arrival order, waiting to be kept or cut at the scan bound.
struct Group {
    rowids: Vec<i64>,
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
        ids: &mut storage::CachedStatement<'_>,
        budget: &EvalBudget,
        taken: &mut Vec<(String, Hit)>,
    ) -> Result<bool, ScanStop> {
        if taken.len() + self.rowids.len() > self.bound {
            let mut keyed = Vec::with_capacity(self.rowids.len());
            for &rowid in &self.rowids {
                budget.check().map_err(|_| ScanStop::Budget)?;
                let id: String = ids.query_row(rusqlite::params![rowid], |row| row.get(0))?;
                keyed.push((id, rowid));
            }
            keyed.sort();
            self.rowids = keyed.into_iter().map(|(_, rowid)| rowid).collect();
        }
        let mut past_bound = false;
        for &rowid in &self.rowids {
            if let Some(found) = live_row(detail, rowid, self.rank, self.ordinal)? {
                if taken.len() == self.bound {
                    past_bound = true;
                    break;
                }
                taken.push(found);
            }
        }
        self.rowids.clear();
        Ok(past_bound)
    }
}

fn hit(row: &rusqlite::Row<'_>, rank: f64, ordinal: usize) -> Result<(String, Hit), ScanStop> {
    let occurrence_id: String = row.get(0)?;
    let class =
        OccurrenceClass::from_code(&row.get::<_, String>(1)?).ok_or(ProjectionError::CorruptRow)?;
    let hit = Hit {
        candidate: OccurrenceCandidate::new(
            occurrence_id.clone(),
            class,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ),
        rank,
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
