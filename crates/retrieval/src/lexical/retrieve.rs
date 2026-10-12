//! Each occurrence keeps its best probe; canonical eligibility is judged in bounded batches before acceptance; accepted contributions are re-judged before return.
//! This module does not read payload bytes; each contribution contains an occurrence identifier and raw FTS rank.
//!
//! [`scan`] reads only the projection: each match's rank and rowid, and the occurrence identifiers of the one rank
//! group the scan bound cuts. [`admit`] reads each rank group's identifiers as it reaches the group and each
//! candidate's occurrence row just before the kernel judges it, both under the same projection connection, so
//! identifiers and occurrence rows are read only for the matches admission reaches; [`retrieve`] runs both.
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

/// One buffer holding every identifier a scan's hits name, so all hits share one growing allocation.
#[derive(Debug, Clone, Default)]
struct Text(String);

impl Text {
    fn push(&mut self, occurrence_id: &str) -> Id {
        let start = self.0.len();
        self.0.push_str(occurrence_id);
        let mut key = [0u8; 8];
        let prefix = &occurrence_id.as_bytes()[..occurrence_id.len().min(8)];
        key[..prefix.len()].copy_from_slice(prefix);
        Id {
            key: u64::from_be_bytes(key),
            span: Span {
                start,
                end: self.0.len(),
            },
        }
    }

    fn get(&self, id: Id) -> &str {
        &self.0[id.span.start..id.span.end]
    }
}

/// One occurrence identifier held in a scan's [`Text`].
#[derive(Debug, Clone, Copy)]
struct Id {
    /// The identifier's first eight bytes, big-endian and zero-padded, which order identifiers as their bytes do whenever the keys differ.
    key: u64,
    span: Span,
}

/// One kept match before its rank group is identified.
#[derive(Debug, Clone, Copy)]
struct Hit {
    /// The probe's raw rank, or [`UNRANKED`] for a common probe's row.
    rank: f64,
    rowid: i64,
    ordinal: usize,
}

/// A kept match whose occurrence identifier has been read; its occurrence row is read at admission.
#[derive(Debug, Clone, Copy)]
struct Named {
    hit: Hit,
    id: Id,
}

/// The kept matches in comparator order. A rank group's identifiers are read when admission reaches the group, except the group the scan bound cut, which the scan identifies to decide which of its members the bound keeps.
#[derive(Debug, Clone, Default)]
struct Kept {
    /// Rank groups not yet identified, in rank order; each group holds the order its scan read.
    pending: VecDeque<Hit>,
    /// Identified hits in comparator order: the pending groups identified so far, then `tail` once `pending` is spent.
    named: Vec<Named>,
    /// The group the scan bound cut, identified, in comparator order, and cut at the bound; it ranks after every pending group.
    tail: Vec<Named>,
    text: Text,
}

impl Kept {
    fn len(&self) -> usize {
        self.named.len() + self.pending.len() + self.tail.len()
    }

    /// Identifies pending rank groups, in order, until `named` holds at least `end` hits or every hit, and returns the VM steps the statements ran.
    fn name_through(
        &mut self,
        conn: &GuardedConn<'_>,
        end: usize,
        budget: &EvalBudget,
    ) -> Result<u64, ScanStop> {
        let mut steps = 0;
        while self.named.len() < end {
            let Some(first) = self.pending.front() else {
                self.named.append(&mut self.tail);
                break;
            };
            let rank = first.rank;
            let size = self
                .pending
                .iter()
                .take_while(|hit| hit.rank.total_cmp(&rank) == Ordering::Equal)
                .count();
            let group: Vec<Hit> = self.pending.drain(..size).collect();
            steps += identify(conn, group, &mut self.text, &mut self.named, budget)?;
        }
        Ok(steps)
    }
}

/// An accepted hit with the facts the kernel judged.
#[derive(Debug, Clone)]
struct Accepted {
    candidate: OccurrenceCandidate,
    rank: f64,
    ordinal: usize,
}

fn by_id(left: Id, right: Id, text: &Text) -> Ordering {
    left.key
        .cmp(&right.key)
        .then_with(|| text.get(left).cmp(text.get(right)))
}

/// Counts a probe's matches in the engine's rowid order inside SQLite, computing no rank; `?2` is the count limit.
const COUNT_SQL: &str = "SELECT count(*) FROM (SELECT rowid FROM lexical WHERE lexical MATCH ?1 ORDER BY rowid LIMIT ?2)";

/// [`scan_ranked`] sorts these matches by rank in memory; `?2` is the probe's match count from [`count_probe`].
const RANKED_SQL: &str =
    "SELECT rank, rowid FROM lexical WHERE lexical MATCH ?1 ORDER BY rowid LIMIT ?2";

/// One page of a common probe's rowids below `?2`, in descending order; `?3` is the page size.
const COMMON_SQL: &str =
    "SELECT rowid FROM lexical WHERE lexical MATCH ?1 AND rowid <= ?2 ORDER BY rowid DESC LIMIT ?3";

/// The content rows from lexical rowid `?1` upward, in rowid order.
/// The index's content table holds each row's columns as `c0`, `c1`, and `c2` in [`super::fts5_table_args`] order, so `c2` is the occurrence identifier; seeking a rowid is one table lookup and the rows after it follow in table order.
const IDS_FROM_SQL: &str = "SELECT id, c2 FROM lexical_content WHERE id >= ?1 ORDER BY id";

/// The most rows [`identify`] steps past between two wanted rowids before it seeks the next one; a step costs about a fifth of a seek, so the allowance tracks how dense the group's rowids are in the table.
const STEP_ALLOWANCE_MAX: u32 = 4;

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
/// Retains the projection connection it was read under and its `RetrievalBounds`, so admission reads identifiers and occurrence rows from the same read transaction and uses the bounds that produced its hits.
#[derive(Clone)]
pub struct Scan<'c> {
    conn: &'c GuardedConn<'c>,
    kept: Kept,
    retrieval: Retrieval,
    bounds: RetrievalBounds,
}

impl Scan<'_> {
    /// Distinct lexical matches kept, before any occurrence row is read or kernel verdict given.
    pub fn hits(&self) -> usize {
        self.kept.len()
    }

    /// The hit occurrences in comparator order, before any kernel verdict; reads the identifiers of every rank group not yet identified.
    ///
    /// # Errors
    ///
    /// Projection errors of the identifier reads; a budget or interrupt that ends them is [`RetrievalRefusal::BudgetExhausted`].
    pub fn hit_ids(&mut self, budget: &EvalBudget) -> Result<Vec<String>, RetrievalRefusal> {
        let end = self.kept.len();
        match self.kept.name_through(self.conn, end, budget) {
            Ok(steps) => self.retrieval.consumed.sql_steps += steps,
            Err(ScanStop::Budget) => return Err(RetrievalRefusal::BudgetExhausted),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        }
        Ok(self
            .kept
            .named
            .iter()
            .map(|named| self.kept.text.get(named.id).to_string())
            .collect())
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
    let mut kept = Kept::default();
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
        scan_common(conn, &common, bounds.scan_rows.get(), budget, &mut kept)
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
        scan_ranked(conn, &ranked, bounds.scan_rows.get(), budget, &mut kept)
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
    retrieval.consumed.scanned_rows = kept.len();
    Ok(Scan {
        conn,
        kept,
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
        mut kept,
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
        &mut kept,
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
        kept: Kept::default(),
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

/// Keeps the request's best `bound` matches across the ranked probes, each given as its ordinal, probe, and counted matches, and returns `true` when more distinct matches exist, with the VM steps the statements ran.
/// Every match of every probe is scored once, the matches merge in rank order, and an occurrence several probes match keeps its lowest rank, then its lowest probe ordinal.
/// Identifiers are read for the rank group the bound cuts, so occurrence identifiers decide which equal-rank matches the bound keeps.
///
/// Each probe must yield matches in ascending rowid order so [`merge_best`] can deduplicate occurrences across probes.
fn scan_ranked(
    conn: &GuardedConn<'_>,
    probes: &[(usize, &Probe, usize)],
    bound: usize,
    budget: &EvalBudget,
    kept: &mut Kept,
) -> Result<(bool, u64), ScanStop> {
    let mut matches: Vec<Hit> = Vec::new();
    let mut steps = 0;
    for &(ordinal, probe, count) in probes {
        let mut rows: Vec<Hit> = Vec::with_capacity(count);
        steps += ranked_rows(conn, probe, ordinal, count, budget, &mut rows)?;
        matches = if matches.is_empty() {
            rows
        } else {
            merge_best(&matches, &rows)
        };
    }
    let by_rank = |left: &Hit, right: &Hit| left.rank.total_cmp(&right.rank);
    if matches.len() <= bound {
        matches.sort_unstable_by(by_rank);
        kept.pending.extend(matches);
        return Ok((false, steps));
    }
    // After the partition, every match below the cut rank precedes the cut element and every match above it follows, while the cut group may straddle it.
    let (_, cut, _) = matches.select_nth_unstable_by(bound - 1, by_rank);
    let cut = cut.rank;
    let mut pending: Vec<Hit> = Vec::with_capacity(bound);
    let mut group: Vec<Hit> = Vec::new();
    for hit in &matches {
        match hit.rank.total_cmp(&cut) {
            Ordering::Less => pending.push(*hit),
            Ordering::Equal => group.push(*hit),
            Ordering::Greater => {}
        }
    }
    drop(matches);
    budget.check().map_err(|_| ScanStop::Budget)?;
    pending.sort_unstable_by(by_rank);
    let start = pending.len();
    kept.pending.extend(pending);
    steps += identify(conn, group, &mut kept.text, &mut kept.tail, budget)?;
    kept.tail.truncate(bound - start);
    Ok((true, steps))
}

/// A lexical row names one occurrence, so the match kept for a rowid is that occurrence's best across the two lists.
fn merge_best(left: &[Hit], right: &[Hit]) -> Vec<Hit> {
    let mut merged: Vec<Hit> = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        match left[i].rowid.cmp(&right[j].rowid) {
            Ordering::Less => {
                merged.push(left[i]);
                i += 1;
            }
            Ordering::Greater => {
                merged.push(right[j]);
                j += 1;
            }
            Ordering::Equal => {
                let (a, b) = (left[i], right[j]);
                let best = match a.rank.total_cmp(&b.rank) {
                    Ordering::Less => a,
                    Ordering::Greater => b,
                    Ordering::Equal if a.ordinal <= b.ordinal => a,
                    Ordering::Equal => b,
                };
                merged.push(best);
                i += 1;
                j += 1;
            }
        }
    }
    merged.extend_from_slice(&left[i..]);
    merged.extend_from_slice(&right[j..]);
    merged
}

/// Appends every match of a ranked probe with its rank, rowid, and the probe's ordinal to `matches` and returns the VM steps the statement ran.
fn ranked_rows(
    conn: &GuardedConn<'_>,
    probe: &Probe,
    ordinal: usize,
    count: usize,
    budget: &EvalBudget,
    matches: &mut Vec<Hit>,
) -> Result<u64, ScanStop> {
    let mut statement = conn.prepare_cached(RANKED_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let mut cursor = statement.query(rusqlite::params![
        probe,
        i64::try_from(count).unwrap_or(i64::MAX)
    ])?;
    while let Some(row) = cursor.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        matches.push(Hit {
            rank: row.get(0)?,
            rowid: row.get(1)?,
            ordinal,
        });
    }
    drop(cursor);
    Ok(vm_steps(&statement))
}

/// Appends one rank group, `group`, to `named` with each lexical row's occurrence identifier and returns the statement's VM step count.
/// The rows are visited in ascending rowid order through one cursor: a wanted rowid near the cursor is reached by stepping past the rows between, a distant one by a fresh seek, and the step allowance adapts to how often each happens.
/// A rowid with no content row is a lexical row the index cannot name.
fn identify(
    conn: &GuardedConn<'_>,
    mut group: Vec<Hit>,
    text: &mut Text,
    named: &mut Vec<Named>,
    budget: &EvalBudget,
) -> Result<u64, ScanStop> {
    let mut statement = conn.prepare_cached(IDS_FROM_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    group.sort_unstable_by_key(|hit| hit.rowid);
    let start = named.len();
    let mut allowance: u32 = STEP_ALLOWANCE_MAX / 2;
    let mut next = 0;
    while next < group.len() {
        // One run: a seek at the next wanted rowid, then steps while wanted rowids stay within the allowance.
        let mut rows = statement.query([group[next].rowid])?;
        let mut sought = true;
        let mut stepped: u32 = 0;
        loop {
            budget.check().map_err(|_| ScanStop::Budget)?;
            let wanted = group[next].rowid;
            let Some(row) = rows.next()? else {
                return Err(ProjectionError::CorruptRow.into());
            };
            match row.get::<_, i64>(0)?.cmp(&wanted) {
                Ordering::Equal => {
                    let id = text.push(str_column(row, 1)?);
                    named.push(Named {
                        hit: group[next],
                        id,
                    });
                    next += 1;
                    if !sought {
                        allowance = (allowance + 1).min(STEP_ALLOWANCE_MAX);
                    }
                    if next == group.len() || allowance == 0 {
                        break;
                    }
                    sought = false;
                    stepped = 0;
                }
                Ordering::Less if !sought && stepped < allowance => stepped += 1,
                Ordering::Less if !sought => {
                    allowance /= 2;
                    break;
                }
                Ordering::Less | Ordering::Greater => {
                    return Err(ProjectionError::CorruptRow.into());
                }
            }
        }
    }
    named[start..].sort_unstable_by(|left, right| {
        by_id(left.id, right.id, text).then(left.hit.ordinal.cmp(&right.hit.ordinal))
    });
    Ok(vm_steps(&statement))
}

/// The `j.key` array index in column 0 of a batched lookup row.
fn key(row: &rusqlite::Row<'_>) -> Result<usize, ScanStop> {
    Ok(usize::try_from(row.get::<_, i64>(0)?).map_err(|_| ProjectionError::CorruptRow)?)
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
    kept: &mut Kept,
) -> Result<(bool, u64), ScanStop> {
    let mut pages = conn.prepare_cached(COMMON_SQL)?;
    pages.reset_status(StatementStatus::VmStep);
    let page_rows = bound.saturating_add(1).min(COMMON_PAGE_ROWS);
    let page = i64::try_from(page_rows).unwrap_or(i64::MAX);
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
    // One row past the bound tells a truncated merge from one that ends exactly at it. The initial capacity is one page, so the upfront allocation stays bounded for a large caller-supplied bound.
    let mut merged: Vec<Hit> = Vec::with_capacity(page_rows);
    while merged.len() <= bound {
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
        merged.push(Hit {
            rank: UNRANKED,
            rowid,
            ordinal,
        });
    }
    let truncated = merged.len() > bound;
    merged.truncate(bound);
    kept.pending.extend(merged);
    Ok((truncated, vm_steps(&pages)))
}

/// Column `index` of `row` as text; any other value fails exactly as `Row::get::<String>` fails.
fn str_column<'r>(row: &'r rusqlite::Row<'_>, index: usize) -> Result<&'r str, ScanStop> {
    if let Ok(text) = row.get_ref(index)?.as_str() {
        return Ok(text);
    }
    row.get::<_, String>(index)?;
    Err(ProjectionError::CorruptRow.into())
}

/// `resolve` returns occurrence rows in `ids` order, uses `None` for missing or tombstoned occurrences, and returns the statement's VM step count.
/// The statement reads the identifiers in ascending order, which keeps successive lookups near each other in the index.
fn resolve(
    conn: &GuardedConn<'_>,
    ids: &[&str],
    budget: &EvalBudget,
) -> Result<(Vec<Option<OccurrenceCandidate>>, u64), ScanStop> {
    let mut statement = conn.prepare_cached(DETAILS_SQL)?;
    statement.reset_status(StatementStatus::VmStep);
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_unstable_by_key(|&index| ids[index]);
    let json = serde_json::to_string(&order.iter().map(|&index| ids[index]).collect::<Vec<_>>())
        .map_err(|_| ProjectionError::CorruptRow)?;
    let mut resolved: Vec<Option<OccurrenceCandidate>> = vec![None; ids.len()];
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
            ids[index].to_string(),
            class,
            text_column(row, 2)?,
            row.get(3)?,
            text_column(row, 4)?,
        ));
    }
    drop(cursor);
    if read != ids.len() {
        return Err(ProjectionError::CorruptRow.into());
    }
    Ok((resolved, vm_steps(&statement)))
}

fn text_column(row: &rusqlite::Row<'_>, index: usize) -> Result<String, ScanStop> {
    Ok(str_column(row, index)?.to_string())
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

/// Judges candidates in comparator order and accepts eligible ones until `max_accepted` fills. The first batch targets the open accepted slots; each later batch scales the open slots by the matches read per accepted occurrence so far. After a batch with no new acceptance, the next target is the larger of that estimate and twice the previous batch. Every batch takes at most `batch_rows` kept matches, reads their occurrence rows, and judges the live ones.
/// An ineligible leader consumes judgment work and no accepted slot, so eligible tails behind it are still reached.
#[allow(clippy::too_many_arguments)]
fn admit_batches(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    authority: Authority<'_>,
    bounds: RetrievalBounds,
    budget: &EvalBudget,
    kept: &mut Kept,
    retrieval: &mut Retrieval,
    hook: &mut impl FnMut(Window),
) -> Result<Vec<Accepted>, AdmitStop> {
    let mut accepted: Vec<Accepted> = Vec::new();
    let mut reached: HashSet<String> = HashSet::new();
    let mut next = 0;
    // `previous` holds the previous batch's kept-match count and the accepted count before that batch ran.
    let mut previous: (usize, usize) = (0, 0);
    while next < kept.len() {
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
        // A batch that adds no accepted occurrence sets the next batch's floor at twice its own size, capped at `batch_rows`, so a run of such batches grows geometrically to whole batches.
        let (previous_rows, accepted_before) = previous;
        let want = match (next, accepted.len()) {
            (0, _) => open,
            (_, 0) => bounds.batch_rows.get(),
            (read, kept) if kept == accepted_before => open
                .saturating_mul(read)
                .div_ceil(kept)
                .max(previous_rows.saturating_mul(2)),
            (read, kept) => open.saturating_mul(read).div_ceil(kept),
        };
        let end = kept
            .len()
            .min(next + want.clamp(1, bounds.batch_rows.get()));
        previous = (end - next, accepted.len());
        retrieval.consumed.sql_steps += kept.name_through(conn, end, budget)?;
        let mut batch: Vec<&Named> = Vec::with_capacity(end - next);
        let mut ids: Vec<&str> = Vec::with_capacity(end - next);
        for named in &kept.named[next..end] {
            let id = kept.text.get(named.id);
            if reached.contains(id) {
                continue;
            }
            reached.insert(id.to_string());
            batch.push(named);
            ids.push(id);
        }
        let (resolved, steps) = resolve(conn, &ids, budget)?;
        retrieval.consumed.sql_steps += steps;
        let mut live: Vec<&Named> = Vec::with_capacity(resolved.len());
        let mut candidates: Vec<OccurrenceCandidate> = Vec::with_capacity(resolved.len());
        for (named, candidate) in batch.into_iter().zip(resolved) {
            match candidate {
                Some(candidate) => {
                    live.push(named);
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
        for ((named, candidate), judged) in live.iter().zip(candidates).zip(report.occurrences) {
            match judged.disposition {
                Disposition::Eligible if accepted.len() == bounds.max_accepted.get() => {
                    incomplete(retrieval, IncompleteReason::AcceptedBound);
                }
                Disposition::Eligible => accepted.push(Accepted {
                    candidate,
                    rank: named.hit.rank,
                    ordinal: named.hit.ordinal,
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
