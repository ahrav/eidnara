//! Each occurrence keeps its best probe; canonical eligibility is judged in bounded batches before acceptance; accepted contributions are re-judged before return.
//! This module does not read payload bytes; each contribution contains an occurrence identifier and raw FTS rank.
//!
//! [`scan`] reads only the projection: each match's rank, rowid, and occurrence identifier. [`admit`] reads each
//! candidate's occurrence row under the same projection connection just before the kernel judges it, so occurrence
//! rows are read only for candidates admission reaches; [`retrieve`] runs both.
//!
//! Ordering is one comparator throughout: lower raw rank first, then occurrence identifier bytes ascending.
//! An occurrence hit by several probes keeps the lowest rank, and among equal ranks the lowest probe ordinal, so duplicate or permuted probes leave the ranking unchanged.
//! A probe repeated later in the request therefore cannot change the ranking, and the engine counts and ranks it only once.
//!
//! Ranking work is bounded before the engine ranks anything. Each distinct probe is first counted in rowid order up to
//! [`RetrievalBounds::qualifying_matches`] plus one lookahead row. Probes with at most
//! [`RetrievalBounds::qualifying_matches`] matches qualify and are ranked in increasing count order, original probe
//! order breaking ties, while the counts of the ranked probes sum to at most [`RetrievalBounds::rank_budget`]. When no
//! probe qualifies, the common probes are read in descending rowid order instead. A skipped probe of either kind leaves
//! the result incomplete.
//!
//! One request keeps at most [`RetrievalBounds::scan_rows`] distinct lexical matches across all of its probes: the
//! ranked probes' matches merge in comparator order, and the common probes' rowid streams merge in descending rowid
//! order. A kept match whose occurrence is missing or tombstoned, which only an inconsistent projection holds, is
//! dropped at admission and no later match replaces it.

use rusqlite::StatementStatus;
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
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
    /// Distinct lexical matches one request keeps, taken across its probes in comparator order for ranked probes and in descending rowid order for common ones.
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
    /// The request's probes matched more distinct rows than its scan bound, so lower-ranked hits were never seen.
    ScanBound,
    /// A probe matched more than [`RetrievalBounds::qualifying_matches`] rows. When every matching probe is over that limit, retrieval merges their rows in descending rowid order and scores each row `0.0`.
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
    /// Distinct lexical matches the request kept within its scan bound; a row several probes match counts once.
    pub scanned_rows: usize,
    /// Kept matches whose occurrence admission found missing or tombstoned, so they were dropped unjudged.
    pub dead_rows: usize,
    /// Rows read to count the distinct probes, lookahead rows included.
    pub counted_rows: usize,
    /// Matches the engine ranked: the summed counts of the ranked probes, at most [`RetrievalBounds::rank_budget`].
    pub ranked_matches: usize,
    pub judged: usize,
    pub batches: usize,
    /// SQLite virtual-machine operations (`SQLITE_STMTSTATUS_VM_STEP`) the count, scan, and identifier statements executed for the probes that finished, plus admission's occurrence reads; the count grows with the rows those statements visit. FTS5's rank function reads each ranked match's document size through a statement FTS5 keeps internal, so that part of scoring is observed through `ranked_matches`.
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

/// The bytes of one string in a scan's [`Text`].
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

/// One buffer holding every string a scan's hits name, so all hits share one growing allocation.
#[derive(Debug, Clone, Default)]
struct Text(String);

impl Text {
    fn push(&mut self, value: &str) -> Span {
        let start = self.0.len();
        self.0.push_str(value);
        Span {
            start,
            end: self.0.len(),
        }
    }

    fn get(&self, span: Span) -> &str {
        &self.0[span.start..span.end]
    }
}

/// One kept match; its occurrence identifier is a span of the scan's [`Text`], and its occurrence row is read at admission.
#[derive(Debug, Clone, Copy)]
struct Hit {
    /// The identifier's first eight bytes, big-endian and zero-padded, which order hits as their identifiers do whenever the keys differ.
    key: u64,
    id: Span,
    rank: f64,
    ordinal: usize,
}

impl Hit {
    fn new(text: &mut Text, occurrence_id: &str, rank: f64, ordinal: usize) -> Self {
        let mut key = [0u8; 8];
        let prefix = &occurrence_id.as_bytes()[..occurrence_id.len().min(8)];
        key[..prefix.len()].copy_from_slice(prefix);
        Self {
            key: u64::from_be_bytes(key),
            id: text.push(occurrence_id),
            rank,
            ordinal,
        }
    }

    fn id<'t>(&self, text: &'t Text) -> &'t str {
        text.get(self.id)
    }
}

/// A scan's hits and the text they point into.
#[derive(Debug, Clone, Default)]
struct Found {
    hits: Vec<Hit>,
    text: Text,
}

/// An accepted hit with the facts the kernel judged.
#[derive(Debug, Clone)]
struct Accepted {
    candidate: OccurrenceCandidate,
    rank: f64,
    ordinal: usize,
}

fn by_id(left: &Hit, right: &Hit, text: &Text) -> Ordering {
    left.key
        .cmp(&right.key)
        .then_with(|| left.id(text).cmp(right.id(text)))
}

/// Sorting with `best_first` lets `dedup_by` retain each occurrence's lowest-rank hit, breaking ties by probe ordinal.
fn best_first(left: &Hit, right: &Hit, text: &Text) -> Ordering {
    by_id(left, right, text)
        .then_with(|| left.rank.total_cmp(&right.rank))
        .then_with(|| left.ordinal.cmp(&right.ordinal))
}

fn comparator(left: &Hit, right: &Hit, text: &Text) -> Ordering {
    left.rank
        .total_cmp(&right.rank)
        .then_with(|| by_id(left, right, text))
}

/// Counts a probe's matches in the engine's rowid order inside SQLite, computing no rank; `?2` is the count limit.
const COUNT_SQL: &str = "SELECT count(*) FROM (SELECT rowid FROM lexical WHERE lexical MATCH ?1 ORDER BY rowid LIMIT ?2)";

/// [`scan_ranked`] sorts these matches by rank in memory; `?2` is the probe's match count from [`count_probe`].
const RANKED_SQL: &str =
    "SELECT rank, rowid FROM lexical WHERE lexical MATCH ?1 ORDER BY rowid LIMIT ?2";

/// One page of a common probe's rowids below `?2`, in descending order; `?3` is the page size.
const COMMON_SQL: &str =
    "SELECT rowid FROM lexical WHERE lexical MATCH ?1 AND rowid <= ?2 ORDER BY rowid DESC LIMIT ?3";

/// `?1` is a JSON array of lexical rowids, and `j.key` is each rowid's index in it.
/// The index's content table holds each row's columns as `c0`, `c1`, and `c2` in [`super::fts5_table_args`] order, so `c2` is the occurrence identifier and reading it by rowid is one table lookup.
const IDS_SQL: &str =
    "SELECT j.key, c.c2 FROM json_each(?1) j JOIN lexical_content c ON c.id=j.value";

/// `?1` is a JSON array of occurrence identifiers, and `j.key` is each identifier's index in it. The occurrence columns are NULL for a missing occurrence and the last column is `false` for a tombstoned one.
/// Under the [`super::index`] invariant that tombstoning deletes the lexical row, such a dead row means an inconsistent projection.
const DETAILS_SQL: &str =
    "SELECT j.key, o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=j.value)
     FROM json_each(?1) j LEFT JOIN occurrences o ON o.occurrence_id=j.value";

/// Rowids one common stream reads per page, so a probe that contributes few rows to the merge reads few rows.
const COMMON_PAGE_ROWS: usize = 512;

/// The raw score a common probe's rows carry: they are read in rowid order, not ranked.
const UNRANKED: f64 = 0.0;

/// The kept matches in comparator order, not yet resolved to occurrence rows or judged by the kernel.
///
/// Retains the projection connection it was read under and its `RetrievalBounds`, so admission reads occurrence rows from the same read transaction and uses the bounds that produced its hits.
#[derive(Clone)]
pub struct Scan<'c> {
    conn: &'c GuardedConn<'c>,
    ordered: Found,
    retrieval: Retrieval,
    bounds: RetrievalBounds,
}

impl Scan<'_> {
    /// Distinct occurrences the kept matches name, before any occurrence row is read or kernel verdict given.
    pub fn hits(&self) -> usize {
        self.ordered.hits.len()
    }

    /// The hit occurrences in comparator order, before any kernel verdict.
    pub fn hit_ids(&self) -> impl Iterator<Item = &str> {
        self.ordered
            .hits
            .iter()
            .map(|hit| hit.id(&self.ordered.text))
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
    admit_inner(scanned, kernel, authority, budget, |_| {})
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
    admit_inner(scanned, kernel, authority, budget, hook)
}

/// Runs every probe against the projection and keeps each occurrence's best hit.
///
/// # Errors
///
/// Refuses bounds the kernel's eligibility batch could never serve, more probes than `max_probes`, and a budget that ends before any probe completes; a budget that ends later yields a [`Scan`] whose admission is already [`Completion::Incomplete`] with no hits.
pub fn scan<'c>(
    conn: &'c GuardedConn<'c>,
    probes: &[Probe],
    bounds: RetrievalBounds,
    budget: &EvalBudget,
) -> Result<Scan<'c>, RetrievalRefusal> {
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
    let mut found = Found::default();
    // Each distinct probe is counted once, under its first ordinal.
    let mut counted: HashSet<&Probe> = HashSet::new();
    let mut distinct: Vec<(usize, &Probe, usize)> = Vec::new();
    for (ordinal, probe) in probes.iter().enumerate() {
        if budget.is_exhausted() {
            return exhausted_scan(conn, retrieval, bounds);
        }
        if !counted.contains(probe) {
            match count_probe(conn, probe, bounds.qualifying_matches, budget) {
                Ok((count, steps)) => {
                    retrieval.consumed.counted_rows += count;
                    retrieval.consumed.sql_steps += steps;
                    counted.insert(probe);
                    distinct.push((ordinal, probe, count));
                }
                Err(ScanStop::Budget) => return exhausted_scan(conn, retrieval, bounds),
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
    let scanned = if qualifying.is_empty() {
        let common: Vec<(usize, &Probe)> = common
            .into_iter()
            .map(|(ordinal, probe, _)| (ordinal, probe))
            .collect();
        scan_common(conn, &common, bounds.scan_rows.get(), budget, &mut found)
    } else {
        let mut ranked = Vec::new();
        for (ordinal, probe, count) in qualifying {
            if retrieval.consumed.ranked_matches + count > bounds.rank_budget.get() {
                incomplete(&mut retrieval, IncompleteReason::RankBudget);
                break;
            }
            retrieval.consumed.ranked_matches += count;
            ranked.push((ordinal, probe, count));
        }
        scan_ranked(conn, &ranked, bounds.scan_rows.get(), budget, &mut found)
    };
    match scanned {
        Ok((truncated, steps)) => {
            retrieval.consumed.sql_steps += steps;
            if truncated {
                incomplete(&mut retrieval, IncompleteReason::ScanBound);
            }
        }
        Err(ScanStop::Budget) => return exhausted_scan(conn, retrieval, bounds),
        Err(ScanStop::Projection(error)) => return Err(error.into()),
    }
    // Both scans keep each rowid once, and a consistent projection holds one lexical row per occurrence, so this dedup removes nothing there;
    // it keeps an inconsistent projection that names one occurrence from two rows from yielding two hits for it.
    // Each hit's identifier, rank, and ordinal together are unique, and each deduplicated hit's identifier is unique, so both orders are total.
    let Found { hits, text } = &mut found;
    hits.sort_unstable_by(|left, right| best_first(left, right, text));
    hits.dedup_by(|later, kept| by_id(later, kept, text) == Ordering::Equal);
    hits.sort_unstable_by(|left, right| comparator(left, right, text));
    retrieval.consumed.scanned_rows = hits.len();
    Ok(Scan {
        conn,
        ordered: found,
        retrieval,
        bounds,
    })
}

/// Reads each kept match's occurrence row as admission reaches it, through the projection connection and read transaction `scanned` was read under, judges the live ones in bounded kernel batches, accepts eligible ones until `max_accepted` fills, and re-judges the accepted set once before returning.
///
/// # Errors
///
/// Returns kernel and projection errors except [`KernelError::Deadline`], which leaves the result [`Completion::Incomplete`] with no contributions.
pub fn admit(
    scanned: Scan<'_>,
    kernel: &KernelStore,
    authority: Authority<'_>,
    budget: &EvalBudget,
) -> Result<Retrieval, RetrievalRefusal> {
    admit_inner(scanned, kernel, authority, budget, |_| {})
}

fn admit_inner(
    scanned: Scan<'_>,
    kernel: &KernelStore,
    authority: Authority<'_>,
    budget: &EvalBudget,
    mut hook: impl FnMut(Window),
) -> Result<Retrieval, RetrievalRefusal> {
    let Scan {
        conn,
        ordered,
        mut retrieval,
        bounds,
    } = scanned;
    if retrieval.completion == Completion::Incomplete(IncompleteReason::BudgetExhausted) {
        return Ok(retrieval);
    }
    let accepted = match admit_batches(
        conn,
        kernel,
        authority,
        bounds,
        budget,
        &ordered,
        &mut retrieval,
        &mut hook,
    ) {
        Ok(accepted) => accepted,
        Err(AdmitStop::Budget) => return exhausted(retrieval),
        Err(AdmitStop::Refused(refusal)) => return Err(refusal),
    };
    hook(Window::BeforeRevalidation);
    revalidate(kernel, authority, budget, accepted, &mut retrieval)?;
    if budget.is_exhausted() {
        return exhausted(retrieval);
    }
    Ok(retrieval)
}

/// Why admission stopped before revalidation.
enum AdmitStop {
    /// The budget ended while an occurrence row was read.
    Budget,
    Refused(RetrievalRefusal),
}

impl From<RetrievalRefusal> for AdmitStop {
    fn from(refusal: RetrievalRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<ScanStop> for AdmitStop {
    fn from(stop: ScanStop) -> Self {
        match stop {
            ScanStop::Budget => Self::Budget,
            ScanStop::Projection(error) => Self::Refused(error.into()),
        }
    }
}

/// A budget-exhausted scan retains no hits; admission returns its recorded completion unchanged.
fn exhausted_scan<'c>(
    conn: &'c GuardedConn<'c>,
    retrieval: Retrieval,
    bounds: RetrievalBounds,
) -> Result<Scan<'c>, RetrievalRefusal> {
    Ok(Scan {
        conn,
        ordered: Found::default(),
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

/// One match before its occurrence is read.
#[derive(Debug, Clone, Copy)]
struct Matched {
    /// The probe's raw rank, or [`UNRANKED`] for a common probe's row.
    rank: f64,
    rowid: i64,
    ordinal: usize,
}

/// Keeps the request's best `bound` matches across the ranked probes, each given as its ordinal, probe, and counted matches, and returns `true` when more distinct matches exist, with the VM steps the statements ran.
/// Every match of every probe is scored once, the matches merge in rank order, and an occurrence several probes match keeps its lowest rank, then its lowest probe ordinal.
/// Identifiers are read for the kept matches and the rest of the last kept rank group, so occurrence identifiers decide which equal-rank matches the bound keeps.
fn scan_ranked(
    conn: &GuardedConn<'_>,
    probes: &[(usize, &Probe, usize)],
    bound: usize,
    budget: &EvalBudget,
    found: &mut Found,
) -> Result<(bool, u64), ScanStop> {
    let total: usize = probes.iter().map(|&(_, _, count)| count).sum();
    let mut matches: Vec<Matched> = Vec::with_capacity(total);
    let mut steps = 0;
    for &(ordinal, probe, count) in probes {
        steps += ranked_rows(conn, probe, ordinal, count, budget, &mut matches)?;
    }
    matches.sort_by(|left, right| {
        left.rank
            .total_cmp(&right.rank)
            .then(left.rowid.cmp(&right.rowid))
            .then(left.ordinal.cmp(&right.ordinal))
    });
    // A lexical row names one occurrence, so the first of a rowid's matches in this order is its best.
    let mut seen = HashSet::with_capacity(matches.len());
    matches.retain(|matched| seen.insert(matched.rowid));
    drop(seen);
    let truncated = matches.len() > bound;
    let mut end = matches.len().min(bound);
    while end < matches.len()
        && matches[end].rank.total_cmp(&matches[end - 1].rank) == Ordering::Equal
    {
        budget.check().map_err(|_| ScanStop::Budget)?;
        end += 1;
    }
    let start = found.hits.len();
    steps += identify(conn, &matches[..end], budget, found)?;
    let Found { hits, text } = found;
    hits[start..].sort_unstable_by(|left, right| {
        comparator(left, right, text).then(left.ordinal.cmp(&right.ordinal))
    });
    hits.truncate(start + bound);
    Ok((truncated, steps))
}

/// Appends every match of a ranked probe with its rank, rowid, and the probe's ordinal to `matches` and returns the VM steps the statement ran.
fn ranked_rows(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    count: usize,
    budget: &EvalBudget,
    matches: &mut Vec<Matched>,
) -> Result<u64, ScanStop> {
    let mut statement = conn.prepare_cached(RANKED_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let mut cursor = statement.query(rusqlite::params![
        probe,
        i64::try_from(count).unwrap_or(i64::MAX)
    ])?;
    while let Some(row) = cursor.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        matches.push(Matched {
            rank: row.get(0)?,
            rowid: row.get(1)?,
            ordinal,
        });
    }
    drop(cursor);
    Ok(vm_steps(&statement))
}

/// Appends a hit for each match, with the occurrence identifier its lexical row names, and returns the VM steps the statement ran.
/// The statement reads the rows in ascending rowid order, which keeps successive lookups near each other in the table.
fn identify(
    conn: &GuardedConn<'_>,
    matches: &[Matched],
    budget: &EvalBudget,
    found: &mut Found,
) -> Result<u64, ScanStop> {
    let mut statement = conn.prepare_cached(IDS_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let mut order: Vec<usize> = (0..matches.len()).collect();
    order.sort_unstable_by_key(|&index| matches[index].rowid);
    let json = json_array(order.iter().map(|&index| matches[index].rowid.to_string()));
    let mut read = 0;
    let mut cursor = statement.query([json])?;
    while let Some(row) = cursor.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        read += 1;
        let index = *order.get(key(row)?).ok_or(ProjectionError::CorruptRow)?;
        let matched = matches[index];
        let hit = Hit::new(
            &mut found.text,
            text(row, 1)?,
            matched.rank,
            matched.ordinal,
        );
        found.hits.push(hit);
    }
    drop(cursor);
    // A rowid with no content row is a lexical row the index cannot name.
    if read != matches.len() {
        return Err(ProjectionError::CorruptRow.into());
    }
    Ok(vm_steps(&statement))
}

/// The `j.key` array index in column 0 of a batched lookup row.
fn key(row: &rusqlite::Row<'_>) -> Result<usize, ScanStop> {
    Ok(usize::try_from(row.get::<_, i64>(0)?).map_err(|_| ProjectionError::CorruptRow)?)
}

fn json_array(items: impl Iterator<Item = String>) -> String {
    let mut json = String::from("[");
    for (index, item) in items.enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&item);
    }
    json.push(']');
    json
}

/// One common probe's rowids in descending order, read a page at a time below the last rowid seen.
struct Descending<'p> {
    ordinal: usize,
    probe: &'p Probe,
    ceiling: i64,
    page: VecDeque<i64>,
    ended: bool,
}

impl Descending<'_> {
    /// The probe's next rowid without consuming it, reading the next page when the current one is spent; a short page ends the match set.
    fn head(
        &mut self,
        statement: &mut storage::CachedStatement<'_>,
        page: i64,
        budget: &EvalBudget,
    ) -> Result<Option<i64>, ScanStop> {
        if self.page.is_empty() && !self.ended {
            let mut rows = statement.query(rusqlite::params![self.probe, self.ceiling, page])?;
            let mut fetched: i64 = 0;
            while let Some(row) = rows.next()? {
                budget.check().map_err(|_| ScanStop::Budget)?;
                fetched += 1;
                let rowid: i64 = row.get(0)?;
                self.ceiling = rowid.saturating_sub(1);
                self.page.push_back(rowid);
            }
            self.ended = fetched < page;
        }
        Ok(self.page.front().copied())
    }
}

/// Keeps the first `bound` distinct rowids of the common probes, each given as its ordinal and probe, in descending rowid order across all of them, and returns `true` when another exists, with the VM steps the statements ran.
/// Each probe's rowids are read a page of at most [`COMMON_PAGE_ROWS`] at a time and merged; a rowid several probes match is kept once, under the lowest of their ordinals.
fn scan_common(
    conn: &GuardedConn<'_>,
    probes: &[(usize, &Probe)],
    bound: usize,
    budget: &EvalBudget,
    found: &mut Found,
) -> Result<(bool, u64), ScanStop> {
    let mut pages = conn.prepare_cached(COMMON_SQL)?;
    pages.reset_status(StatementStatus::VmStep);
    let page = i64::try_from(bound.saturating_add(1).min(COMMON_PAGE_ROWS)).unwrap_or(i64::MAX);
    let mut streams: Vec<Descending<'_>> = probes
        .iter()
        .map(|&(ordinal, probe)| Descending {
            ordinal,
            probe,
            ceiling: i64::MAX,
            page: VecDeque::new(),
            ended: false,
        })
        .collect();
    // One row past the bound tells a truncated merge from one that ends exactly at it.
    let mut kept: Vec<Matched> = Vec::with_capacity(bound + 1);
    while kept.len() <= bound {
        let mut next: Option<(i64, usize)> = None;
        for stream in &mut streams {
            if let Some(rowid) = stream.head(&mut pages, page, budget)? {
                // Streams are in ordinal order, so the first stream holding the greatest rowid has the lowest ordinal.
                if next.is_none_or(|(best, _)| rowid > best) {
                    next = Some((rowid, stream.ordinal));
                }
            }
        }
        let Some((rowid, ordinal)) = next else {
            break;
        };
        for stream in &mut streams {
            if stream.page.front() == Some(&rowid) {
                stream.page.pop_front();
            }
        }
        kept.push(Matched {
            rank: UNRANKED,
            rowid,
            ordinal,
        });
    }
    let truncated = kept.len() > bound;
    kept.truncate(bound);
    let steps = identify(conn, &kept, budget, found)?;
    Ok((truncated, vm_steps(&pages) + steps))
}

/// Column `index` of `row` as text; any other value fails exactly as `Row::get::<String>` fails.
fn text<'r>(row: &'r rusqlite::Row<'_>, index: usize) -> Result<&'r str, ScanStop> {
    if let Ok(text) = row.get_ref(index)?.as_str() {
        return Ok(text);
    }
    row.get::<_, String>(index)?;
    Err(ProjectionError::CorruptRow.into())
}

/// The occurrence row of each of `hits`, in their order, with `None` for a missing or tombstoned occurrence, and the VM steps the statement ran.
/// The statement reads the identifiers in ascending order, which keeps successive lookups near each other in the index.
fn resolve(
    conn: &GuardedConn<'_>,
    hits: &[Hit],
    text: &Text,
    budget: &EvalBudget,
) -> Result<(Vec<Option<OccurrenceCandidate>>, u64), ScanStop> {
    let mut statement = conn.prepare_cached(DETAILS_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_unstable_by(|&left, &right| hits[left].id(text).cmp(hits[right].id(text)));
    let json = serde_json::to_string(
        &order
            .iter()
            .map(|&index| hits[index].id(text))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| ProjectionError::CorruptRow)?;
    let mut resolved: Vec<Option<OccurrenceCandidate>> = vec![None; hits.len()];
    let mut read = 0;
    let mut cursor = statement.query([json])?;
    while let Some(row) = cursor.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        read += 1;
        let index = *order.get(key(row)?).ok_or(ProjectionError::CorruptRow)?;
        // Column 1 is NULL for a missing occurrence and column 5 is `false` for a tombstoned one.
        let present = row.get_ref(1)? != rusqlite::types::ValueRef::Null;
        let untombstoned: bool = row.get(5)?;
        if !(present && untombstoned) {
            continue;
        }
        let class = row
            .get_ref(1)?
            .as_str()
            .ok()
            .and_then(OccurrenceClass::from_code)
            .ok_or(ProjectionError::CorruptRow)?;
        resolved[index] = Some(OccurrenceCandidate::new(
            hits[index].id(text).to_string(),
            class,
            text_column(row, 2)?,
            row.get(3)?,
            text_column(row, 4)?,
        ));
    }
    drop(cursor);
    if read != hits.len() {
        return Err(ProjectionError::CorruptRow.into());
    }
    Ok((resolved, vm_steps(&statement)))
}

fn text_column(row: &rusqlite::Row<'_>, index: usize) -> Result<String, ScanStop> {
    Ok(text(row, index)?.to_string())
}

/// Returns `SnapshotChanged` or `KernelIncarnationChanged` if kernel state differs from the first batch,
/// and `None` when the budget ended during the judgment, which is then already recorded on `retrieval`.
fn judge_batch(
    kernel: &KernelStore,
    authority: Authority<'_>,
    candidates: &[OccurrenceCandidate],
    budget: &EvalBudget,
    retrieval: &mut Retrieval,
) -> Result<Option<(EligibilityReport, Option<IncompleteReason>)>, RetrievalRefusal> {
    let (report, moved) = match judge_tracked(
        kernel,
        authority,
        candidates,
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
    retrieval.consumed.judged += candidates.len();
    let moved = moved.map(|moved| match moved {
        AuthorityMoved::Incarnation => IncompleteReason::KernelIncarnationChanged,
        AuthorityMoved::Snapshot => IncompleteReason::SnapshotChanged,
    });
    Ok(Some((report, moved)))
}

/// Judges candidates in comparator order and accepts eligible ones until `max_accepted` fills. The first batch takes the open accepted slots; each later batch takes the open slots scaled by the matches read per accepted occurrence so far, so a mostly eligible window reads few occurrences past the bound and a sparse one still advances a whole batch at a time. Every batch takes at most `batch_rows` kept matches, reads their occurrence rows, and judges the live ones.
/// An ineligible leader consumes judgment work and no accepted slot, so eligible tails behind it are still reached.
#[allow(clippy::too_many_arguments)]
fn admit_batches(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    authority: Authority<'_>,
    bounds: RetrievalBounds,
    budget: &EvalBudget,
    ordered: &Found,
    retrieval: &mut Retrieval,
    hook: &mut impl FnMut(Window),
) -> Result<Vec<Accepted>, AdmitStop> {
    let mut accepted: Vec<Accepted> = Vec::new();
    let mut next = 0;
    while next < ordered.hits.len() {
        if accepted.len() == bounds.max_accepted.get() {
            incomplete(retrieval, IncompleteReason::AcceptedBound);
            break;
        }
        if budget.is_exhausted() {
            incomplete(retrieval, IncompleteReason::BudgetExhausted);
            break;
        }
        let open = bounds.max_accepted.get() - accepted.len();
        // The matches read per accepted occurrence so far predict how many fill the open slots; a batch that has accepted none yet takes a whole batch.
        let want = match (next, accepted.len()) {
            (0, _) => open,
            (_, 0) => bounds.batch_rows.get(),
            (read, kept) => open.saturating_mul(read).div_ceil(kept),
        };
        let end = ordered
            .hits
            .len()
            .min(next + want.clamp(1, bounds.batch_rows.get()));
        let (resolved, steps) = resolve(conn, &ordered.hits[next..end], &ordered.text, budget)?;
        retrieval.consumed.sql_steps += steps;
        let mut batch: Vec<&Hit> = Vec::with_capacity(resolved.len());
        let mut candidates: Vec<OccurrenceCandidate> = Vec::with_capacity(resolved.len());
        for (hit, candidate) in ordered.hits[next..end].iter().zip(resolved) {
            match candidate {
                Some(candidate) => {
                    batch.push(hit);
                    candidates.push(candidate);
                }
                None => retrieval.consumed.dead_rows += 1,
            }
        }
        next = end;
        if candidates.is_empty() {
            continue;
        }
        let Some((report, moved)) = judge_batch(kernel, authority, &candidates, budget, retrieval)?
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
        for ((hit, candidate), judged) in batch.iter().zip(candidates).zip(report.occurrences) {
            match judged.disposition {
                Disposition::Eligible if accepted.len() == bounds.max_accepted.get() => {
                    incomplete(retrieval, IncompleteReason::AcceptedBound);
                }
                Disposition::Eligible => accepted.push(Accepted {
                    candidate,
                    rank: hit.rank,
                    ordinal: hit.ordinal,
                }),
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
    accepted: Vec<Accepted>,
    retrieval: &mut Retrieval,
) -> Result<(), RetrievalRefusal> {
    if accepted.is_empty() {
        return Ok(());
    }
    if budget.is_exhausted() {
        incomplete(retrieval, IncompleteReason::BudgetExhausted);
        return Ok(());
    }
    let candidates: Vec<OccurrenceCandidate> =
        accepted.iter().map(|hit| hit.candidate.clone()).collect();
    let Some((report, moved)) = judge_batch(kernel, authority, &candidates, budget, retrieval)?
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
