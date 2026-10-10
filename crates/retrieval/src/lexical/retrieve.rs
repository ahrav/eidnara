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
    /// SQLite virtual-machine operations (`SQLITE_STMTSTATUS_VM_STEP`) the count, scan, and lookup statements executed for the probes that finished; the count grows with the rows those statements visit. FTS5's rank function reads each ranked match's document size through a statement FTS5 keeps internal, so that part of scoring is observed through `ranked_matches`.
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

/// One probe hit; its occurrence identifier, object identifier, and artifact digest are spans of the scan's [`Text`].
#[derive(Debug, Clone, Copy)]
struct Hit {
    /// The identifier's first eight bytes, big-endian and zero-padded, which order hits as their identifiers do whenever the keys differ.
    key: u64,
    id: Span,
    object: Span,
    digest: Span,
    class: OccurrenceClass,
    revision: i64,
    rank: f64,
    ordinal: usize,
}

impl Hit {
    fn id<'t>(&self, text: &'t Text) -> &'t str {
        text.get(self.id)
    }

    fn candidate(&self, text: &Text) -> OccurrenceCandidate {
        OccurrenceCandidate::new(
            self.id(text).to_string(),
            self.class,
            text.get(self.object).to_string(),
            self.revision,
            text.get(self.digest).to_string(),
        )
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

/// [`ranked_rows`] sorts these matches by rank in memory; `?2` is the probe's match count from [`count_probe`].
const RANKED_SQL: &str =
    "SELECT rank, rowid FROM lexical WHERE lexical MATCH ?1 ORDER BY rowid LIMIT ?2";

/// Each lexical row joins its occurrence: the occurrence columns are NULL for a missing occurrence and the last column is `false` for a tombstoned one.
/// Under the [`super::index`] invariant that tombstoning deletes the lexical row, such a dead row means an inconsistent projection.
const WHOLE_SQL: &str = "SELECT l.rank, l.occurrence_id, o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=l.occurrence_id)
     FROM lexical l LEFT JOIN occurrences o ON o.occurrence_id=l.occurrence_id
     WHERE lexical MATCH ?1 ORDER BY l.rowid LIMIT ?2";

/// Joins each row's occurrence as [`WHOLE_SQL`] does.
const COMMON_SQL: &str = "SELECT l.rowid, l.occurrence_id, o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=l.occurrence_id)
     FROM lexical l LEFT JOIN occurrences o ON o.occurrence_id=l.occurrence_id
     WHERE lexical MATCH ?1 AND l.rowid <= ?2 ORDER BY l.rowid DESC LIMIT ?3";

/// `?1` is a JSON array of lexical rowids, and `j.key` is each rowid's index in it.
const IDS_SQL: &str =
    "SELECT j.key, l.occurrence_id FROM json_each(?1) j JOIN lexical l ON l.rowid=j.value";

/// `?1` is a JSON array of occurrence identifiers, and `j.key` is each identifier's index in it; the occurrence columns read as in [`WHOLE_SQL`].
const DETAILS_SQL: &str =
    "SELECT j.key, o.class, o.source_object_id, o.revision, o.source_artifact_digest,
            NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=j.value)
     FROM json_each(?1) j LEFT JOIN occurrences o ON o.occurrence_id=j.value";

/// The raw score a common probe's rows carry: they are read in rowid order, not ranked.
const UNRANKED: f64 = 0.0;

/// Probe hits read from the projection in comparator order, not yet judged by the kernel.
///
/// Retains its `RetrievalBounds` so admission uses the bounds that produced its hits.
#[derive(Debug, Clone)]
pub struct Scan {
    ordered: Found,
    retrieval: Retrieval,
    bounds: RetrievalBounds,
}

impl Scan {
    /// Distinct occurrences the probes hit, before any kernel verdict.
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
    let mut found = Found::default();
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
            &mut found,
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
    // Each hit's identifier, rank, and ordinal together are unique, and each deduplicated hit's identifier is unique, so both orders are total.
    let Found { hits, text } = &mut found;
    hits.sort_unstable_by(|left, right| best_first(left, right, text));
    hits.dedup_by(|later, kept| by_id(later, kept, text) == Ordering::Equal);
    hits.sort_unstable_by(|left, right| comparator(left, right, text));
    Ok(Scan {
        ordered: found,
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
        &ordered,
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

/// The probe scan appends its best `scan_rows` live rows to `found`; the returned boolean is `true` when additional live rows exist past the bound.
fn scan_probe(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    run: Run,
    scan_rows: NonZeroUsize,
    budget: &EvalBudget,
    found: &mut Found,
) -> Result<(usize, bool, u64), ScanStop> {
    let bound = scan_rows.get();
    let mut taken = Taken {
        hits: Vec::new(),
        text: &mut found.text,
    };
    let (truncated, steps) = match run {
        Run::Common => scan_common(conn, probe, ordinal, bound, budget, &mut taken)?,
        // Every live match fits the bound, so the scan keeps each one and the probe ends within its bound.
        Run::Ranked(count) if count <= bound => (
            false,
            scan_whole(conn, probe, ordinal, count, budget, &mut taken)?,
        ),
        Run::Ranked(count) => scan_ranked(conn, probe, ordinal, count, bound, budget, &mut taken)?,
    };
    let seen = taken.hits.len();
    found.hits.append(&mut taken.hits);
    Ok((seen, truncated, steps))
}

/// Keeps every live match of a probe whose `count` matches fit the scan bound and returns the VM steps its statement ran.
fn scan_whole(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    count: usize,
    budget: &EvalBudget,
    taken: &mut Taken<'_>,
) -> Result<u64, ScanStop> {
    let mut statement = conn.prepare_cached(WHOLE_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let mut rows = statement.query(rusqlite::params![
        probe,
        i64::try_from(count).unwrap_or(i64::MAX)
    ])?;
    while let Some(row) = rows.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        taken.offer(row, 2, text(row, 1)?, row.get(0)?, ordinal)?;
    }
    drop(rows);
    Ok(vm_steps(&statement))
}

/// Keeps the best `bound` live matches of a probe with more matches than the bound, in rank order and then occurrence identifier order, and returns `true` when a live match lies past the bound, with the VM steps its statements ran.
/// Identifiers are read for the rows the bound can still reach, extended to the end of the last rank among them, so occurrence identifiers decide which equal-rank rows the bound keeps.
/// Occurrences are read for the rows the bound still needs plus one, which shows whether a live row lies past it.
fn scan_ranked(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    count: usize,
    bound: usize,
    budget: &EvalBudget,
    taken: &mut Taken<'_>,
) -> Result<(bool, u64), ScanStop> {
    let (rows, ranked_steps) = ranked_rows(conn, probe, count, budget)?;
    let mut ids = conn.prepare_cached(IDS_SQL)?;
    ids.reset_status(StatementStatus::VmStep);
    let mut details = conn.prepare_cached(DETAILS_SQL)?;
    details.reset_status(StatementStatus::VmStep);
    let mut next = 0;
    let mut pending: VecDeque<(f64, String)> = VecDeque::new();
    let truncated = loop {
        budget.check().map_err(|_| ScanStop::Budget)?;
        let wanted = bound - taken.hits.len() + 1;
        if pending.is_empty() {
            if next == rows.len() {
                break false;
            }
            let mut end = rows.len().min(next + wanted);
            while end < rows.len() && rows[end].0.total_cmp(&rows[end - 1].0) == Ordering::Equal {
                end += 1;
            }
            let mut identified = identify(&mut ids, &rows[next..end])?;
            identified.sort_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
            });
            pending.extend(identified);
            next = end;
        }
        let batch: Vec<(f64, String)> = pending.drain(..pending.len().min(wanted)).collect();
        if resolve(&mut details, &batch, ordinal, bound, taken)? {
            break true;
        }
    };
    Ok((
        truncated,
        ranked_steps + vm_steps(&ids) + vm_steps(&details),
    ))
}

/// Every match of a ranked probe with its rank and rowid, ordered by rank and then rowid, with the VM steps the statement ran.
fn ranked_rows(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    count: usize,
    budget: &EvalBudget,
) -> Result<(Vec<(f64, i64)>, u64), ScanStop> {
    let mut statement = conn.prepare_cached(RANKED_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let mut rows = Vec::with_capacity(count);
    let mut cursor = statement.query(rusqlite::params![
        probe,
        i64::try_from(count).unwrap_or(i64::MAX)
    ])?;
    while let Some(row) = cursor.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        rows.push((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?));
    }
    drop(cursor);
    let steps = vm_steps(&statement);
    rows.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
    Ok((rows, steps))
}

/// Each row's rank with its occurrence identifier, in the order of `rows`.
/// The statement reads the rows in ascending rowid order, which keeps successive lookups near each other in the index.
fn identify(
    statement: &mut storage::CachedStatement<'_>,
    rows: &[(f64, i64)],
) -> Result<Vec<(f64, String)>, ScanStop> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_unstable_by_key(|&index| rows[index].1);
    let json = json_array(order.iter().map(|&index| rows[index].1.to_string()));
    let mut found: Vec<Option<String>> = vec![None; rows.len()];
    let mut cursor = statement.query([json])?;
    while let Some(row) = cursor.next()? {
        let slot = found
            .get_mut(order[key(row)?])
            .ok_or(ProjectionError::CorruptRow)?;
        *slot = Some(row.get(1)?);
    }
    rows.iter()
        .zip(found)
        .map(|(&(rank, _), id)| Ok((rank, id.ok_or(ProjectionError::CorruptRow)?)))
        .collect()
}

/// Offers each of `batch`'s occurrences to `taken` in the order of `batch` and returns `true` when a live one lies past `bound`.
/// The statement reads the identifiers in ascending order, which keeps successive lookups near each other in the index.
fn resolve(
    statement: &mut storage::CachedStatement<'_>,
    batch: &[(f64, String)],
    ordinal: usize,
    bound: usize,
    taken: &mut Taken<'_>,
) -> Result<bool, ScanStop> {
    let mut order: Vec<usize> = (0..batch.len()).collect();
    order.sort_unstable_by(|&left, &right| batch[left].1.cmp(&batch[right].1));
    let json = serde_json::to_string(
        &order
            .iter()
            .map(|&index| batch[index].1.as_str())
            .collect::<Vec<_>>(),
    )
    .map_err(|_| ProjectionError::CorruptRow)?;
    let mut found: Vec<Option<Hit>> = vec![None; batch.len()];
    let mut seen = vec![false; batch.len()];
    let start = taken.hits.len();
    let mut cursor = statement.query([json])?;
    while let Some(row) = cursor.next()? {
        let index = *order.get(key(row)?).ok_or(ProjectionError::CorruptRow)?;
        if std::mem::replace(&mut seen[index], true) {
            return Err(ProjectionError::CorruptRow.into());
        }
        let (rank, occurrence_id) = &batch[index];
        if taken.offer(row, 1, occurrence_id, *rank, ordinal)? {
            found[index] = taken.hits.pop();
        }
    }
    drop(cursor);
    debug_assert_eq!(taken.hits.len(), start);
    for hit in found.into_iter().flatten() {
        if taken.hits.len() == bound {
            return Ok(true);
        }
        taken.hits.push(hit);
    }
    Ok(false)
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

/// `scan_common` takes a common probe's first `bound` live rows in descending rowid order and returns `true` when another live row exists, with the VM steps its page statement ran.
/// Each page reads `bound + 1` rows below the last rowid seen, so a dead row costs one read and no kept slot, and a short page ends the match set.
fn scan_common(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    bound: usize,
    budget: &EvalBudget,
    taken: &mut Taken<'_>,
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
            if taken.hits.len() == bound {
                if live(row, 2)? {
                    break 'pages true;
                }
            } else {
                taken.offer(row, 2, text(row, 1)?, UNRANKED, ordinal)?;
            }
        }
        if fetched < page {
            break false;
        }
    };
    Ok((truncated, vm_steps(&statement)))
}

/// Whether a joined row names a live occurrence; the occurrence columns start at `at`, with the tombstone test four columns later.
fn live(row: &rusqlite::Row<'_>, at: usize) -> Result<bool, ScanStop> {
    Ok(row.get_ref(at)? != rusqlite::types::ValueRef::Null && row.get::<_, bool>(at + 4)?)
}

/// Column `index` of `row` as text; any other value fails exactly as `Row::get::<String>` fails.
fn text<'r>(row: &'r rusqlite::Row<'_>, index: usize) -> Result<&'r str, ScanStop> {
    if let Ok(text) = row.get_ref(index)?.as_str() {
        return Ok(text);
    }
    row.get::<_, String>(index)?;
    Err(ProjectionError::CorruptRow.into())
}

/// The hits one probe keeps, with the text they point into.
struct Taken<'t> {
    hits: Vec<Hit>,
    text: &'t mut Text,
}

impl Taken<'_> {
    /// Keeps the live occurrence a joined row names and returns `true`, or returns `false` for a dead row; the occurrence columns start at `at` as for [`live`].
    fn offer(
        &mut self,
        row: &rusqlite::Row<'_>,
        at: usize,
        occurrence_id: &str,
        rank: f64,
        ordinal: usize,
    ) -> Result<bool, ScanStop> {
        if !live(row, at)? {
            return Ok(false);
        }
        let class = row
            .get_ref(at)?
            .as_str()
            .ok()
            .and_then(OccurrenceClass::from_code)
            .ok_or(ProjectionError::CorruptRow)?;
        let mut key = [0u8; 8];
        let prefix = &occurrence_id.as_bytes()[..occurrence_id.len().min(8)];
        key[..prefix.len()].copy_from_slice(prefix);
        let id = self.text.push(occurrence_id);
        let object = self.text.push(text(row, at + 1)?);
        let revision = row.get(at + 2)?;
        let digest = self.text.push(text(row, at + 3)?);
        self.hits.push(Hit {
            key: u64::from_be_bytes(key),
            id,
            object,
            digest,
            class,
            revision,
            rank,
            ordinal,
        });
        Ok(true)
    }
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

/// Judges candidates in comparator order, `batch_rows` at a time, and accepts eligible ones until `max_accepted` fills.
/// An ineligible leader consumes judgment work and no accepted slot, so eligible tails behind it are still reached.
fn admit_batches(
    kernel: &KernelStore,
    authority: Authority<'_>,
    bounds: RetrievalBounds,
    budget: &EvalBudget,
    ordered: &Found,
    retrieval: &mut Retrieval,
    hook: &mut impl FnMut(Window),
) -> Result<Vec<Accepted>, RetrievalRefusal> {
    let mut accepted: Vec<Accepted> = Vec::new();
    for batch in ordered.hits.chunks(bounds.batch_rows.get()) {
        if accepted.len() == bounds.max_accepted.get() {
            incomplete(retrieval, IncompleteReason::AcceptedBound);
            break;
        }
        if budget.is_exhausted() {
            incomplete(retrieval, IncompleteReason::BudgetExhausted);
            break;
        }
        let candidates: Vec<OccurrenceCandidate> = batch
            .iter()
            .map(|hit| hit.candidate(&ordered.text))
            .collect();
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
