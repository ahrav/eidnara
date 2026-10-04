//! Walks every live occurrence whose class requires a vector in occurrence identifier order through bounded keyset pages, inside the caller's read transaction.
//! Each page is validated and scored as it is read, in blocks of eight rows; only the rows that would enter the current top-K are judged for canonical eligibility, in rank order and in batches sized so a page whose k best rows are eligible judges only those k, and the eligible ones are offered; the top-K is re-judged once before return.
//! Judging only rows that can enter the set returns the same rows as judging every row: a member of the final top-K outranks the worst held member at every earlier point of the walk, so it is never skipped.
//! A live required row without a vector is a coverage shortfall, so the result is incomplete even when every scored row was eligible; a kernel snapshot or incarnation that moves between batches ends the walk the same way.
//! The walk itself is shared: a `RowSource` supplies the page query and the vector of each visited row, so the oracle reads `occurrence_vectors` and the layered ranking reads resolved layer rows through one judgment, admission, and revalidation path.
//! A ranked walk visits the same population in rowid order, scores every visited row once its last page is read, and judges those rows best first across the whole walk until `k` eligible rows are held or the rows run out; a set that fills was judged over the eligible top-K, every excluded row ranked above its last member, and the rest of the batch that filled it, at most `page_rows` rows.
//! Those batches run after every page under one snapshot, so a ranked walk returns its held set without a re-judgment.

use std::num::NonZeroUsize;
use std::ops::ControlFlow;
use std::sync::LazyLock;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CommitReadIncarnation, EgressSnapshot, EligibilityCandidate, EligibilityVerdict, KernelError,
    KernelStore, MAX_ELIGIBILITY_CANDIDATES,
};
use rusqlite::params;
use storage::GuardedConn;

use super::codec::{self, Metric, RowLayout, RowRejection};
use super::score::{BLOCK_ROWS, Ranked, TopK, rank_order, score_block};
use crate::ProjectionError;
use crate::batch::{VectorGeneration, dense_eligible};
use crate::coverage::CURRENT_PENDING;
use crate::eligibility::{
    Authority, AuthorityMoved, Disposition, EligibilityReport, OccurrenceCandidate, judge_tracked,
    tally_exclusion,
};
use crate::scan::ScanStop;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OracleBounds {
    /// Rows returned; at most [`MAX_ELIGIBILITY_CANDIDATES`] so the final re-judgment fits one batch.
    pub k: NonZeroUsize,
    /// Rows read, validated, and scored per page; at most [`MAX_ELIGIBILITY_CANDIDATES`] so the rows a page selects for judgment fit one batch.
    pub page_rows: NonZeroUsize,
    /// Live required rows one request may visit before it stops as incomplete.
    pub max_rows: NonZeroUsize,
}

/// Not `Debug`: the query row is embedding content.
pub struct ExhaustiveQuery<'a> {
    pub generation: &'a VectorGeneration,
    pub metric: Metric,
    /// The generation's `unit_norm_tolerance`; the daemon supplies the inference owner's value.
    pub unit_norm_tolerance: f64,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub bounds: OracleBounds,
}

impl<'a> ExhaustiveQuery<'a> {
    fn walk(&self) -> Walk<'a> {
        Walk {
            generation: self.generation,
            layout: RowLayout {
                dimension: self.generation.vector_dimension,
                metric: self.metric,
                unit_norm_tolerance: self.unit_norm_tolerance,
            },
            query: self.query,
            authority: self.authority,
            bounds: self.bounds,
        }
    }
}

/// What every walk needs of a request, whichever source supplies the rows.
pub(super) struct Walk<'a> {
    pub generation: &'a VectorGeneration,
    pub layout: RowLayout,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub bounds: OracleBounds,
}

/// Byte limits on the two stores a ranked walk fills, counted separately: the temporary candidates of one judgment batch, and the accepted set.
/// The walk also keeps one score and key per scored row until its judgment ends; `max_rows` bounds that store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageBounds {
    /// Bytes of the candidates one judgment batch holds, each counted as [`selected_bytes`] counts it. A batch holds at most `page_rows` rows, so the bound covers `page_rows` rows at their largest; a batch ends early at the row that would exceed it, and a row that fits no batch stops the walk. The kernel's verdict report for a batch is the kernel's own allocation and stays outside the count.
    pub batch_bytes: NonZeroUsize,
    /// Bytes of the accepted set while the walk runs: its preallocated entries plus the strings each admitted entry owns, as [`held_bytes`] counts them. The result moves the same entries through two more vectors of at most `k` slots.
    pub heap_bytes: NonZeroUsize,
}

/// One row of a judgment batch: the candidate, its score, and the strings the candidate owns.
pub fn selected_bytes(candidate: &OccurrenceCandidate) -> usize {
    SELECTED_ROW_BYTES + candidate_strings(candidate)
}

const SELECTED_ROW_BYTES: usize = size_of::<OccurrenceCandidate>() + size_of::<f64>();

/// The strings one held entry owns beyond its preallocated slot: the candidate's, plus the ranked copy of its identifier.
pub fn held_bytes(candidate: &OccurrenceCandidate) -> usize {
    candidate_strings(candidate) + candidate.occurrence_id.len()
}

/// The slot the accepted set preallocates per entry.
pub const HELD_SLOT_BYTES: usize = size_of::<(Ranked, OccurrenceCandidate)>();

fn candidate_strings(candidate: &OccurrenceCandidate) -> usize {
    strings_of(
        &candidate.occurrence_id,
        &candidate.candidate.object_id,
        candidate.candidate.artifact_digest.as_deref().unwrap_or(""),
    )
}

fn strings_of(occurrence_id: &str, object_id: &str, digest: &str) -> usize {
    occurrence_id.len() + object_id.len() + digest.len()
}

/// Supplies the live required rows in identifier order, what each carries into scoring, and its score.
pub(super) trait RowSource {
    /// What one visited row carries into scoring, held in a lane that is reused row after row.
    type Payload: Default;

    /// The page query, in the shape `Progress::visit_page` documents.
    fn page_sql(&self) -> &str;

    /// Writes what the visited row carries into `into`; `false` when the source holds nothing for the row.
    fn load(
        &mut self,
        row: &PageRow<'_>,
        layout: &RowLayout,
        into: &mut Self::Payload,
    ) -> Result<bool, OracleRefusal>;

    /// Validates the lanes in visit order and writes one score per lane into `scores`; the first lane that fails refuses, leaving `scores` with exactly the lanes before it, so the caller checks those lanes' identities ahead of the refusal.
    fn score(
        &self,
        request: &Walk<'_>,
        lanes: &[Lane<Self::Payload>],
        scores: &mut Vec<f64>,
    ) -> Result<(), OracleRefusal>;

    /// Runs after every page whose rows were all visited, with whether rows remain past it.
    fn after_page(&mut self, _more: bool) {}
}

/// Supplies a ranked walk's rows: which visited rows it holds, and their scores once the walk has visited every page.
pub(super) trait RankedSource {
    /// The page query in rowid order over the rows [`PAGE_SQL`] selects: the rowid, the occurrence identifier, the source object identifier, and the source artifact digest, binding the generation identifier as `?1`, the first rowid to visit as `?2`, and the limit as `?3`.
    fn page_sql(&self) -> &str;

    /// Records the visited row and its rowid; `false` when the source holds nothing for it.
    fn visit(&mut self, occurrence_id: &str, rowid: i64) -> bool;

    /// Runs after every page whose rows were all visited, with whether rows remain past it.
    fn after_page(&mut self, more: bool);

    /// Validates and scores every recorded row, pushing its score and key into `scored`; the first row that fails refuses. `false` when the budget ended first.
    fn score(
        &mut self,
        budget: &EvalBudget,
        scored: &mut Vec<(f64, usize)>,
    ) -> Result<bool, OracleRefusal>;

    /// The occurrence identifier of the row a key names.
    fn occurrence_id(&self, key: usize) -> &str;

    /// The rowid [`Self::visit`] recorded for the row a key names.
    fn rowid(&self, key: usize) -> i64;
}

/// Scores original f32 lanes in blocks of [`BLOCK_ROWS`] and validates each from the sum of squares its block produced. A block short of `BLOCK_ROWS` repeats its first row in the spare lanes; their results are discarded.
pub(super) fn score_originals(
    request: &Walk<'_>,
    lanes: &[Lane<Vec<f32>>],
    scores: &mut Vec<f64>,
) -> Result<(), OracleRefusal> {
    scores.clear();
    for block in lanes.chunks(BLOCK_ROWS) {
        let rows: [&[f32]; BLOCK_ROWS] =
            std::array::from_fn(
                |lane| &block[if lane < block.len() { lane } else { 0 }].payload[..],
            );
        let sums = score_block(request.layout.metric, request.query, &rows);
        for (lane, (sum_of_squares, score)) in block
            .iter()
            .zip(sums.sums_of_squares.into_iter().zip(sums.scores))
        {
            codec::validate_from_sum(&lane.payload, &request.layout, sum_of_squares).map_err(
                |rejection| OracleRefusal::StoredRow {
                    occurrence_id: lane.occurrence_id.clone(),
                    rejection,
                },
            )?;
            scores.push(score);
        }
    }
    Ok(())
}

/// The projection's own `occurrence_vectors` of the generation.
struct StoredVectors;

impl RowSource for StoredVectors {
    type Payload = Vec<f32>;

    fn page_sql(&self) -> &str {
        &PAGE_SQL
    }

    fn score(
        &self,
        request: &Walk<'_>,
        lanes: &[Lane<Vec<f32>>],
        scores: &mut Vec<f64>,
    ) -> Result<(), OracleRefusal> {
        score_originals(request, lanes, scores)
    }

    fn load(
        &mut self,
        row: &PageRow<'_>,
        layout: &RowLayout,
        into: &mut Vec<f32>,
    ) -> Result<bool, OracleRefusal> {
        let Some(bytes) = row.stored else {
            return Ok(false);
        };
        codec::decode_length_into(bytes, layout.dimension, into).map_err(|rejection| {
            OracleRefusal::StoredRow {
                occurrence_id: row.occurrence_id.to_owned(),
                rejection,
            }
        })?;
        Ok(true)
    }
}

/// Counts over the visited prefix of `R`, the live rows whose class requires a vector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DenseCoverage {
    pub required: usize,
    /// Members of `R` whose stored vector decoded and satisfied the layout.
    pub with_vector: usize,
    /// Members of `R` without a vector of the generation while durable work for it is still open.
    pub missing_pending: usize,
    /// Members of `R` without a vector and without open work.
    pub missing_without_pending: usize,
}

impl DenseCoverage {
    pub fn missing(&self) -> usize {
        self.missing_pending + self.missing_without_pending
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteReason {
    /// A live required row has no vector of the generation; the ranking omits it and cannot stand for the population.
    DenseCoverageShortfall,
    /// `max_rows` were visited while rows remained.
    RowBound,
    /// A row would exceed the walk's batch bytes in a judgment batch of its own.
    BatchBytes,
    /// An admission would take the accepted set past the walk's heap bytes.
    HeapBytes,
    BudgetExhausted,
    KernelIncarnationChanged,
    /// The kernel snapshot moved between eligibility batches, so later verdicts describe other facts.
    SnapshotChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// Every live required row was visited and carried a valid vector; every judged row, including the returned set, was judged under one snapshot.
    /// Only rows that could enter the top-K when visited were judged, so completion describes the ranking, not a policy verdict on every row.
    Complete,
    Incomplete(IncompleteReason),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Consumed {
    pub pages: usize,
    /// Rows the kernel judged: those that could have entered the top-K when visited, plus the final re-judgment; for a ranked source, those judged best first until the top-K filled.
    pub judged: usize,
    /// One or more per page that selected a row, plus the re-judgment; a page with no row that could enter the top-K runs none. A ranked source runs one per batch after the walk.
    pub batches: usize,
    /// Rows the kernel judged ineligible before or at final revalidation, by verdict in judgment order.
    pub excluded: Vec<(EligibilityVerdict, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExhaustiveRanking {
    /// Re-judged rows, best first, at most `k`.
    pub ranked: Vec<Ranked>,
    /// The terms each `ranked` row was judged under, in `ranked` order, so a later revalidation judges the same facts without another projection read.
    pub candidates: Vec<OccurrenceCandidate>,
    pub completion: Completion,
    pub coverage: DenseCoverage,
    /// The snapshot every ranked row was judged under; `None` if no batch ran.
    pub snapshot: Option<EgressSnapshot>,
    pub incarnation: Option<CommitReadIncarnation>,
    pub consumed: Consumed,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OracleRefusal {
    #[error(
        "the {bound} bound of {value} exceeds the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate batch"
    )]
    BatchOverBound { bound: &'static str, value: usize },
    #[error("the query row is not a member of the generation: {0}")]
    Query(RowRejection),
    #[error(
        "the stored vector of occurrence {occurrence_id} is not a member of the generation: {rejection}"
    )]
    StoredRow {
        occurrence_id: String,
        rejection: RowRejection,
    },
    /// A row source holds a row for the occurrence but could not produce it; the projection's own vectors never raise this.
    #[error("the vector of occurrence {occurrence_id} could not be read: {detail}")]
    Unreadable {
        occurrence_id: String,
        detail: String,
    },
    #[error("the request's budget ended before any page was read")]
    BudgetExhausted,
    #[error(
        "{entries} preallocated entries of {HELD_SLOT_BYTES} bytes exceed the {limit}-byte heap bound"
    )]
    HeapOverBound { entries: usize, limit: usize },
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window<'a> {
    /// A live required row is about to be decoded.
    Visited(&'a str),
    AfterJudgment,
    AfterPage(usize),
    BeforeRevalidation,
}

/// One visited row, borrowed from the page statement while it is positioned on it.
pub(super) struct PageRow<'r> {
    pub occurrence_id: &'r str,
    pub class: OccurrenceClass,
    pub source_object_id: &'r str,
    pub revision: i64,
    pub source_artifact_digest: &'r str,
    /// The projection's stored vector bytes, when the page query selects them.
    pub stored: Option<&'r [u8]>,
    /// `Some(true)` when durable work for the row's vector is still open. A page query that selects `NULL` yields `None`, and the walk reads [`PENDING_SQL`] once the source reports no row.
    pub pending: Option<bool>,
}

/// The quoted codes of every class that requires a vector, for an SQL `IN` list.
pub(super) static DENSE_CLASSES: LazyLock<String> = LazyLock::new(|| {
    OccurrenceClass::ALL
        .into_iter()
        .filter(|class| dense_eligible(*class))
        .map(|class| format!("'{}'", class.code()))
        .collect::<Vec<_>>()
        .join(",")
});

/// One keyset over the primary key covers every dense class, so no page sorts a class and visit order equals identifier order.
/// The unary `+` on `o.class` keeps the planner off the class index, which would sort the whole class on every page.
/// The unary `+` on the limit keeps the plan independent of the bound value, so rebinding it for the next page reuses the prepared statement.
static PAGE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest,v.vector,
                CASE WHEN v.vector IS NULL THEN {CURRENT_PENDING} ELSE 0 END
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         LEFT JOIN occurrence_vectors v ON v.occurrence_id=o.occurrence_id AND v.generation_id=?1
         WHERE t.occurrence_id IS NULL AND +o.class IN ({}) AND o.occurrence_id>?2
         ORDER BY o.occurrence_id
         LIMIT +?3",
        *DENSE_CLASSES
    )
});

/// Rows a ranked walk visits between two reads of the budget.
const BUDGET_STRIDE: usize = 16;

/// The identity fields of one occurrence, which a ranked walk already visited as live inside the same transaction.
const CANDIDATE_SQL: &str = "SELECT occurrence_id,class,source_object_id,revision,source_artifact_digest FROM occurrences WHERE rowid=?1";

/// [`CURRENT_PENDING`] for one occurrence, bound like a page query: the generation identifier as `?1` and the occurrence identifier as `?2`.
static PENDING_SQL: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT {CURRENT_PENDING} FROM occurrences o WHERE o.occurrence_id=?2")
});

fn pending_of(
    conn: &GuardedConn<'_>,
    generation: &VectorGeneration,
    occurrence_id: &str,
) -> Result<bool, ScanStop> {
    let mut statement = conn.prepare_cached(&PENDING_SQL)?;
    let pending: i64 = statement
        .query_row(params![&generation.generation_id, occurrence_id], |row| {
            row.get(0)
        })?;
    Ok(pending != 0)
}

/// # Errors
///
/// A budget that ends before any page is read is [`OracleRefusal::BudgetExhausted`]; one that ends later leaves the result [`Completion::Incomplete`] with no ranked rows.
/// A stored vector that fails the layout refuses the whole request as [`OracleRefusal::StoredRow`]; no row of its page is judged or returned, though rows of that page visited before it were scored and are discarded.
pub fn exhaustive(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &ExhaustiveQuery<'_>,
    budget: &EvalBudget,
) -> Result<ExhaustiveRanking, OracleRefusal> {
    walk(
        conn,
        kernel,
        &request.walk(),
        budget,
        &mut StoredVectors,
        |_| {},
    )
}

/// `hook` runs before each visited row is decoded, after every page, and once before the final re-judgment, so a test can change the kernel or the budget in those windows.
#[cfg(feature = "test-support")]
pub fn exhaustive_with_hook_for_test(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &ExhaustiveQuery<'_>,
    budget: &EvalBudget,
    hook: impl FnMut(Window<'_>),
) -> Result<ExhaustiveRanking, OracleRefusal> {
    walk(
        conn,
        kernel,
        &request.walk(),
        budget,
        &mut StoredVectors,
        hook,
    )
}

/// The checks every walk makes before it reads a row: the budget, the batch limit on `k` and `page_rows`, the layout and the query, the held set's slots against `storage`, and the generation.
fn check_request(
    conn: &GuardedConn<'_>,
    request: &Walk<'_>,
    storage: Option<StorageBounds>,
    budget: &EvalBudget,
) -> Result<(), OracleRefusal> {
    budget.check().map_err(|_| OracleRefusal::BudgetExhausted)?;
    let bounds = request.bounds;
    for (bound, value) in [("k", bounds.k.get()), ("page_rows", bounds.page_rows.get())] {
        if value > MAX_ELIGIBILITY_CANDIDATES {
            return Err(OracleRefusal::BatchOverBound { bound, value });
        }
    }
    let layout = request.layout;
    layout.check().map_err(OracleRefusal::Query)?;
    codec::validate(request.query, &layout).map_err(OracleRefusal::Query)?;
    if let Some(storage) = storage {
        let entries = bounds.k.get();
        if entries.saturating_mul(HELD_SLOT_BYTES) > storage.heap_bytes.get() {
            return Err(OracleRefusal::HeapOverBound {
                entries,
                limit: storage.heap_bytes.get(),
            });
        }
    }
    crate::vectors::check_generation(conn, request.generation)?;
    Ok(())
}

/// Walks `source`'s live required rows under `request`'s bounds; see [`exhaustive`] for the result's meaning.
pub(super) fn walk(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &Walk<'_>,
    budget: &EvalBudget,
    source: &mut impl RowSource,
    hook: impl FnMut(Window<'_>),
) -> Result<ExhaustiveRanking, OracleRefusal> {
    check_request(conn, request, None, budget)?;
    let bounds = request.bounds;
    let mut progress = Progress::new(bounds, None, hook);
    loop {
        if budget.is_exhausted() {
            return exhausted(progress.ranking);
        }
        let take = bounds
            .page_rows
            .get()
            .min(bounds.max_rows.get() - progress.ranking.coverage.required);
        let Some(page) = progress.visit_page(conn, request, source, take, budget)? else {
            return progress.ended(budget);
        };
        let more = page.more;
        if let Some(selected) = page.selected {
            source.after_page(more);
            let flow = progress.judge_selected(kernel, request.authority, selected, budget)?;
            (progress.hook)(Window::AfterPage(progress.ranking.consumed.pages));
            match flow {
                ControlFlow::Break(Some(moved)) => {
                    incomplete(&mut progress.ranking, moved_reason(moved));
                    break;
                }
                ControlFlow::Break(None) => return progress.ended(budget),
                ControlFlow::Continue(()) => {}
            }
        } else {
            source.after_page(more);
        }
        if !more {
            break;
        }
        if progress.ranking.coverage.required == bounds.max_rows.get() {
            incomplete(&mut progress.ranking, IncompleteReason::RowBound);
            break;
        }
    }
    (progress.hook)(Window::BeforeRevalidation);
    let Progress {
        mut ranking, top, ..
    } = progress;
    revalidate(kernel, request.authority, budget, top, &mut ranking)?;
    finish(ranking, budget)
}

/// Discards a result the budget ended under, and names a coverage shortfall; a walk that stopped early already names the stronger reason, so a shortfall is only the reason when the walk otherwise completed.
fn finish(
    mut ranking: ExhaustiveRanking,
    budget: &EvalBudget,
) -> Result<ExhaustiveRanking, OracleRefusal> {
    if budget.is_exhausted() {
        return exhausted(ranking);
    }
    if ranking.coverage.missing() != 0 {
        incomplete(&mut ranking, IncompleteReason::DenseCoverageShortfall);
    }
    Ok(ranking)
}

/// Walks `source`'s live required rows in rowid order under `request`'s bounds, scores them once every page is read, and judges them best first; see [`exhaustive`] for the result's meaning.
/// Identity fields are validated as each row is visited and the source's rows when they are scored, so a corrupt identity refuses the request before any score does.
/// A walk that stops before its last page scores and judges nothing; it reports the rows it visited as required and none with a vector.
pub(super) fn walk_ranked(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &Walk<'_>,
    storage: StorageBounds,
    budget: &EvalBudget,
    source: &mut impl RankedSource,
    hook: impl FnMut(Window<'_>),
) -> Result<ExhaustiveRanking, OracleRefusal> {
    check_request(conn, request, Some(storage), budget)?;
    let bounds = request.bounds;
    let mut progress: Progress<(), _> = Progress::new(bounds, Some(storage), hook);
    loop {
        if budget.is_exhausted() {
            return exhausted(progress.ranking);
        }
        let take = bounds
            .page_rows
            .get()
            .min(bounds.max_rows.get() - progress.ranking.coverage.required);
        let Some(more) = progress.visit_ranked_page(conn, request, source, take, budget)? else {
            return progress.ended(budget);
        };
        source.after_page(more);
        (progress.hook)(Window::AfterPage(progress.ranking.consumed.pages));
        if !more {
            break;
        }
        if progress.ranking.coverage.required == bounds.max_rows.get() {
            incomplete(&mut progress.ranking, IncompleteReason::RowBound);
            break;
        }
    }
    if progress.ranking.completion == Completion::Complete {
        let mut scored = Vec::new();
        if !source.score(budget, &mut scored)? {
            return exhausted(progress.ranking);
        }
        progress.ranking.coverage.with_vector = scored.len();
        match progress.judge_ranked(conn, kernel, request, source, scored, budget)? {
            ControlFlow::Break(Some(moved)) => {
                incomplete(&mut progress.ranking, moved_reason(moved));
            }
            ControlFlow::Break(None) => return progress.ended(budget),
            ControlFlow::Continue(()) => {}
        }
    }
    // Every batch ran after the last page under the first batch's snapshot, so the held set is as fresh as a re-judgment would make it.
    let Progress {
        mut ranking, top, ..
    } = progress;
    let (rows, candidates): (Vec<Ranked>, Vec<OccurrenceCandidate>) =
        top.into_ranked().into_iter().unzip();
    ranking.ranked = rows;
    ranking.candidates = candidates;
    finish(ranking, budget)
}

/// Rows admitted before the budget ended were never re-judged under it, so none is returned.
fn exhausted(ranking: ExhaustiveRanking) -> Result<ExhaustiveRanking, OracleRefusal> {
    if ranking.consumed.pages == 0 {
        return Err(OracleRefusal::BudgetExhausted);
    }
    Ok(discarded(ranking, IncompleteReason::BudgetExhausted))
}

/// A walk that stopped mid-page holds a set nobody re-judged, so the result keeps the counts and returns no row.
fn discarded(mut ranking: ExhaustiveRanking, reason: IncompleteReason) -> ExhaustiveRanking {
    drop_rows(&mut ranking);
    incomplete(&mut ranking, reason);
    ranking
}

pub(super) fn drop_rows(ranking: &mut ExhaustiveRanking) {
    ranking.ranked.clear();
    ranking.candidates.clear();
}

/// The bytes a ranked walk's stores hold, and the storage bound that stopped it.
#[derive(Default)]
struct Stored {
    /// Bytes of the current judgment batch.
    selected: usize,
    /// String bytes of the accepted set's entries.
    held: usize,
    stopped: Option<IncompleteReason>,
    limits: Option<StorageBounds>,
}

impl Stored {
    /// The candidate and score slots a judgment batch of `want` rows reserves: every candidate the batch accepts costs at least [`SELECTED_ROW_BYTES`] of its batch bytes, so the reserved slots fit inside the bound and the batch never grows them.
    fn batch_slots(&self, want: usize) -> usize {
        self.limits.map_or(want, |storage| {
            want.min(storage.batch_bytes.get() / SELECTED_ROW_BYTES)
        })
    }
}

/// The first reason stands, except that an ended budget always wins: whatever was known before, the result was not finished.
fn incomplete(ranking: &mut ExhaustiveRanking, reason: IncompleteReason) {
    if reason == IncompleteReason::BudgetExhausted || ranking.completion == Completion::Complete {
        ranking.completion = Completion::Incomplete(reason);
    }
}

/// Rows of one page that could enter the top-K, with their scores in the same order.
#[derive(Default)]
struct Selected {
    candidates: Vec<OccurrenceCandidate>,
    scores: Vec<f64>,
}

struct Page {
    /// Rows remain past this page.
    more: bool,
    /// `None` when the page held no row.
    selected: Option<Selected>,
}

/// One row of a scoring block, copied out of the page statement into buffers that are reused block after block; it becomes an owned candidate only when the top-K admits it.
#[derive(Default)]
pub(super) struct Lane<P> {
    pub occurrence_id: String,
    class: Option<OccurrenceClass>,
    source_object_id: String,
    revision: i64,
    source_artifact_digest: String,
    pub payload: P,
}

impl<P> Lane<P> {
    fn record(&mut self, row: &PageRow<'_>) {
        self.occurrence_id.clear();
        self.occurrence_id.push_str(row.occurrence_id);
        self.class = Some(row.class);
        self.source_object_id.clear();
        self.source_object_id.push_str(row.source_object_id);
        self.revision = row.revision;
        self.source_artifact_digest.clear();
        self.source_artifact_digest
            .push_str(row.source_artifact_digest);
    }

    fn candidate(&self) -> OccurrenceCandidate {
        OccurrenceCandidate::new(
            self.occurrence_id.clone(),
            self.class.expect("a recorded row has a class"),
            self.source_object_id.clone(),
            self.revision,
            self.source_artifact_digest.clone(),
        )
    }
}

/// Up to [`BLOCK_ROWS`] visited rows with a payload, validated and scored together.
struct Block<P> {
    lanes: Vec<Lane<P>>,
    filled: usize,
    scores: Vec<f64>,
}

impl<P: Default> Block<P> {
    fn new() -> Self {
        Self {
            lanes: Vec::with_capacity(BLOCK_ROWS),
            filled: 0,
            scores: Vec::with_capacity(BLOCK_ROWS),
        }
    }

    /// Obtains the row's payload from `source` into the next lane and records the row's identity behind it; `false` when the source holds nothing for the row.
    fn push<S: RowSource<Payload = P>>(
        &mut self,
        row: &PageRow<'_>,
        source: &mut S,
        layout: &RowLayout,
    ) -> Result<bool, OracleRefusal> {
        if self.filled == self.lanes.len() {
            self.lanes.push(Lane::default());
        }
        let lane = &mut self.lanes[self.filled];
        if !source.load(row, layout, &mut lane.payload)? {
            return Ok(false);
        }
        lane.record(row);
        self.filled += 1;
        Ok(true)
    }

    fn is_full(&self) -> bool {
        self.filled == BLOCK_ROWS
    }

    /// Validates and scores the buffered rows in visit order, exactly as scoring each row alone would: the first row that fails refuses, every validated row counts toward coverage, and only then is its identity validated and its score offered to `top`.
    fn flush<S: RowSource<Payload = P>>(
        &mut self,
        request: &Walk<'_>,
        source: &S,
        ranking: &mut ExhaustiveRanking,
        top: &TopK<OccurrenceCandidate>,
        selected: &mut Selected,
    ) -> Result<(), OracleRefusal> {
        let filled = std::mem::take(&mut self.filled);
        if filled == 0 {
            return Ok(());
        }
        let scored = source.score(request, &self.lanes[..filled], &mut self.scores);
        for (lane, score) in self.lanes[..filled].iter().zip(&self.scores) {
            ranking.coverage.with_vector += 1;
            EligibilityCandidate::validate_fields(
                &lane.source_object_id,
                Some(&lane.source_artifact_digest),
            )?;
            if !top.admits(*score, &lane.occurrence_id) {
                continue;
            }
            selected.candidates.push(lane.candidate());
            selected.scores.push(*score);
        }
        scored
    }
}

/// The walk's state between pages: the result under construction, the held set, the admission counts that size judgment batches, the identifier to resume after or the rowid to resume from, the scoring block, the stores a ranked walk bounds, and the test hook.
struct Progress<P, H> {
    ranking: ExhaustiveRanking,
    top: TopK<OccurrenceCandidate>,
    admissions: Admissions,
    after: String,
    /// The first rowid a ranked walk's next page may visit.
    from_rowid: i64,
    block: Block<P>,
    stored: Stored,
    hook: H,
}

impl<P: Default, H: FnMut(Window<'_>)> Progress<P, H> {
    fn new(bounds: OracleBounds, storage: Option<StorageBounds>, hook: H) -> Self {
        Self {
            ranking: ExhaustiveRanking {
                ranked: Vec::new(),
                candidates: Vec::new(),
                completion: Completion::Complete,
                coverage: DenseCoverage::default(),
                snapshot: None,
                incarnation: None,
                consumed: Consumed::default(),
            },
            top: TopK::new(bounds.k),
            admissions: Admissions::default(),
            after: String::new(),
            from_rowid: i64::MIN,
            block: Block::new(),
            stored: Stored {
                limits: storage,
                ..Stored::default()
            },
            hook,
        }
    }

    /// The budget ended or a storage bound stopped the walk; an ended budget wins, as in [`incomplete`].
    fn ended(self, budget: &EvalBudget) -> Result<ExhaustiveRanking, OracleRefusal> {
        match self.stored.stopped {
            Some(reason) if !budget.is_exhausted() => Ok(discarded(self.ranking, reason)),
            _ => exhausted(self.ranking),
        }
    }

    /// Steps one page of at most `take` rows plus one probe row past it, copying each row into a reused scoring lane while the statement is positioned on it, so nothing is allocated for a row that cannot enter the top-K.
    /// The source's SQL selects the seven columns of [`PAGE_SQL`] in that order and binds the generation identifier, the identifier to start after, and the limit as `?1`, `?2`, `?3`; `after` is left at the last visited identifier.
    /// Rows are validated and scored in blocks of up to [`BLOCK_ROWS`]: when a block is full, when the page ends, and before an ended budget or a later row's error is acted on, so the rows visited before it are judged for the layout exactly as they would have been one at a time.
    /// A row's identity fields are validated before `top.admits` is consulted, so a corrupt row is refused even when it could not enter the top-K; a missing vector is counted and its row is neither judged nor scored.
    /// `None` means the budget ended inside the page; `ranking` then describes the rows visited before it did.
    fn visit_page<S: RowSource<Payload = P>>(
        &mut self,
        conn: &GuardedConn<'_>,
        request: &Walk<'_>,
        source: &mut S,
        take: usize,
        budget: &EvalBudget,
    ) -> Result<Option<Page>, OracleRefusal> {
        let limit = i64::try_from(take.saturating_add(1)).unwrap_or(i64::MAX);
        let mut statement = match conn
            .prepare_cached(source.page_sql())
            .map_err(ScanStop::from)
        {
            Ok(statement) => statement,
            Err(ScanStop::Budget) => return Ok(None),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        };
        let mut rows = match statement
            .query(params![
                &request.generation.generation_id,
                self.after.as_str(),
                limit
            ])
            .map_err(ScanStop::from)
        {
            Ok(rows) => rows,
            Err(ScanStop::Budget) => return Ok(None),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        };
        let mut selected = Selected::default();
        let mut visited = 0usize;
        let more = loop {
            let row = match rows.next().map_err(ScanStop::from) {
                Ok(Some(row)) => row,
                Ok(None) => break false,
                Err(ScanStop::Budget) => {
                    return self.flush(request, source, &mut selected).and(Ok(None));
                }
                Err(ScanStop::Projection(error)) => {
                    return self
                        .flush(request, source, &mut selected)
                        .and(Err(error.into()));
                }
            };
            if budget.check().is_err() {
                return self.flush(request, source, &mut selected).and(Ok(None));
            }
            if visited == take {
                break true;
            }
            let row = match borrow_row(row) {
                Ok(row) => row,
                Err(error) => return self.flush(request, source, &mut selected).and(Err(error)),
            };
            if visited == 0 {
                self.ranking.consumed.pages += 1;
            }
            (self.hook)(Window::Visited(row.occurrence_id));
            if budget.is_exhausted() {
                return self.flush(request, source, &mut selected).and(Ok(None));
            }
            visited += 1;
            self.after.clear();
            self.after.push_str(row.occurrence_id);
            self.ranking.coverage.required += 1;
            match self.block.push(&row, source, &request.layout) {
                Ok(true) => {
                    if self.block.is_full() {
                        self.flush(request, source, &mut selected)?;
                    }
                }
                Ok(false) => {
                    let pending = match row.pending {
                        Some(pending) => Ok(pending),
                        None => pending_of(conn, request.generation, row.occurrence_id),
                    };
                    match pending {
                        Ok(true) => self.ranking.coverage.missing_pending += 1,
                        Ok(false) => self.ranking.coverage.missing_without_pending += 1,
                        // Coverage counts rows whose disposition is known.
                        Err(ScanStop::Budget) => {
                            self.ranking.coverage.required -= 1;
                            return self.flush(request, source, &mut selected).and(Ok(None));
                        }
                        Err(ScanStop::Projection(error)) => {
                            return self
                                .flush(request, source, &mut selected)
                                .and(Err(error.into()));
                        }
                    }
                }
                // The buffered rows were visited before this one, so a rejection among them refuses the request instead of this error.
                Err(error) => return self.flush(request, source, &mut selected).and(Err(error)),
            }
        };
        self.flush(request, source, &mut selected)?;
        Ok(Some(Page {
            more,
            selected: (visited > 0).then_some(selected),
        }))
    }

    fn flush<S: RowSource<Payload = P>>(
        &mut self,
        request: &Walk<'_>,
        source: &S,
        selected: &mut Selected,
    ) -> Result<(), OracleRefusal> {
        self.block
            .flush(request, source, &mut self.ranking, &self.top, selected)
    }

    /// Steps one page of a ranked walk: at most `take` rows plus one probe row past it, in rowid order, recording each row the source holds and validating that row's identity fields; a row the source does not hold is counted as missing, as in [`Self::visit_page`]. `from_rowid` is left just past the last visited rowid.
    /// `Some(more)` says whether rows remain past the page; `None` means the budget ended inside it.
    fn visit_ranked_page(
        &mut self,
        conn: &GuardedConn<'_>,
        request: &Walk<'_>,
        source: &mut impl RankedSource,
        take: usize,
        budget: &EvalBudget,
    ) -> Result<Option<bool>, OracleRefusal> {
        let limit = i64::try_from(take.saturating_add(1)).unwrap_or(i64::MAX);
        let mut statement = match conn
            .prepare_cached(source.page_sql())
            .map_err(ScanStop::from)
        {
            Ok(statement) => statement,
            Err(ScanStop::Budget) => return Ok(None),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        };
        let mut rows = match statement
            .query(params![
                &request.generation.generation_id,
                self.from_rowid,
                limit
            ])
            .map_err(ScanStop::from)
        {
            Ok(rows) => rows,
            Err(ScanStop::Budget) => return Ok(None),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        };
        let mut visited = 0usize;
        loop {
            let row = match rows.next().map_err(ScanStop::from) {
                Ok(Some(row)) => row,
                Ok(None) => return Ok(Some(false)),
                Err(ScanStop::Budget) => return Ok(None),
                Err(ScanStop::Projection(error)) => return Err(error.into()),
            };
            if visited == take {
                return Ok(Some(true));
            }
            let column = |index: usize| row.get_ref(index).map_err(ProjectionError::from);
            let text = |index: usize| -> Result<&str, OracleRefusal> {
                Ok(column(index)?
                    .as_str()
                    .map_err(|_| ProjectionError::CorruptRow)?)
            };
            let rowid = column(0)?
                .as_i64()
                .map_err(|_| ProjectionError::CorruptRow)?;
            let occurrence_id = text(1)?;
            if visited == 0 {
                self.ranking.consumed.pages += 1;
            }
            (self.hook)(Window::Visited(occurrence_id));
            // The deadline is read every few rows rather than every row; an ended budget still stops the walk within a block of rows.
            if visited.is_multiple_of(BUDGET_STRIDE) && budget.is_exhausted() {
                return Ok(None);
            }
            visited += 1;
            // A row past the largest rowid cannot exist, so a saturated bound is never queried again.
            self.from_rowid = rowid.saturating_add(1);
            self.ranking.coverage.required += 1;
            if source.visit(occurrence_id, rowid) {
                EligibilityCandidate::validate_fields(text(2)?, Some(text(3)?))?;
                continue;
            }
            let pending = pending_of(conn, request.generation, occurrence_id);
            match pending {
                Ok(true) => self.ranking.coverage.missing_pending += 1,
                Ok(false) => self.ranking.coverage.missing_without_pending += 1,
                // Coverage counts rows whose disposition is known.
                Err(ScanStop::Budget) => {
                    self.ranking.coverage.required -= 1;
                    return Ok(None);
                }
                Err(ScanStop::Projection(error)) => return Err(error.into()),
            }
        }
    }

    /// Judges a ranked walk's scored rows best first across the whole walk, a batch at a time, until the set holds `k` eligible rows or the rows run out.
    /// Each batch is sized to the rows expected to fill the set at the observed admission rate, at most `page_rows` rows and the batch bytes; its rows' identity fields are read again at the rowids the walk saw them at, inside the caller's transaction.
    fn judge_ranked(
        &mut self,
        conn: &GuardedConn<'_>,
        kernel: &KernelStore,
        request: &Walk<'_>,
        source: &impl RankedSource,
        rows: Vec<(f64, usize)>,
        budget: &EvalBudget,
    ) -> Result<ControlFlow<Option<AuthorityMoved>>, OracleRefusal> {
        // `rank_order` with the identifiers read only for tied scores, so most comparisons touch no winner.
        let order = |left: &(f64, usize), right: &(f64, usize)| {
            right.0.total_cmp(&left.0).then_with(|| {
                source
                    .occurrence_id(left.1)
                    .as_bytes()
                    .cmp(source.occurrence_id(right.1).as_bytes())
            })
        };
        let k = self.top.k();
        let cap = request.bounds.page_rows.get();
        let mut unjudged = Unjudged::new(rows, order);
        // Drawn rows not yet judged, best first; each ranks ahead of every row still in `unjudged`.
        let mut drawn = Vec::new();
        let mut eligible = 0usize;
        loop {
            let remaining = drawn.len() + unjudged.len();
            // Rows are judged best first, so a full set ranks ahead of every remaining row and admits none of them.
            if remaining == 0 || self.top.is_full() {
                break;
            }
            if budget.is_exhausted() {
                return Ok(ControlFlow::Break(None));
            }
            let want = self
                .admissions
                .ranked_batch(k - eligible.min(k), remaining)
                .min(cap);
            unjudged.draw(&mut drawn, want);
            self.stored.selected = 0;
            let slots = self.stored.batch_slots(want);
            let mut candidates = Vec::with_capacity(slots);
            let mut scores = Vec::with_capacity(slots);
            for &(score, key) in &drawn[..want] {
                match self.read_candidate(conn, source.occurrence_id(key), source.rowid(key)) {
                    Ok(Some(candidate)) => {
                        candidates.push(candidate);
                        scores.push(score);
                    }
                    Ok(None) if candidates.is_empty() => {
                        self.stored.stopped = Some(IncompleteReason::BatchBytes);
                        return Ok(ControlFlow::Break(None));
                    }
                    Ok(None) => break,
                    Err(ScanStop::Budget) => return Ok(ControlFlow::Break(None)),
                    Err(ScanStop::Projection(error)) => return Err(error.into()),
                }
            }
            let taken = candidates.len();
            let before = self.admissions.eligible;
            let flow = self.judge_batch(kernel, request.authority, candidates, scores, budget)?;
            if flow.is_break() {
                return Ok(flow);
            }
            eligible += self.admissions.eligible - before;
            drawn.drain(..taken);
        }
        Ok(ControlFlow::Continue(()))
    }

    /// The candidate of a live row the walk visited, read again at the rowid the walk saw it at inside the same transaction; `None` when it would take the batch past the batch bytes.
    fn read_candidate(
        &mut self,
        conn: &GuardedConn<'_>,
        occurrence_id: &str,
        rowid: i64,
    ) -> Result<Option<OccurrenceCandidate>, ScanStop> {
        let mut statement = conn.prepare_cached(CANDIDATE_SQL)?;
        let mut rows = statement.query(params![rowid])?;
        let row = rows.next()?.ok_or(ProjectionError::CorruptRow)?;
        let text = |index: usize| -> Result<&str, ScanStop> {
            Ok(row
                .get_ref(index)
                .map_err(ProjectionError::from)?
                .as_str()
                .map_err(|_| ProjectionError::CorruptRow)?)
        };
        if text(0)? != occurrence_id {
            return Err(ProjectionError::CorruptRow.into());
        }
        let class = OccurrenceClass::from_code(text(1)?).ok_or(ProjectionError::CorruptRow)?;
        let (object_id, digest) = (text(2)?, text(4)?);
        let revision = row
            .get_ref(3)
            .map_err(ProjectionError::from)?
            .as_i64()
            .map_err(|_| ProjectionError::CorruptRow)?;
        if let Some(storage) = self.stored.limits {
            let bytes = self.stored.selected
                + SELECTED_ROW_BYTES
                + strings_of(occurrence_id, object_id, digest);
            if bytes > storage.batch_bytes.get() {
                return Ok(None);
            }
            self.stored.selected = bytes;
        }
        Ok(Some(OccurrenceCandidate::new(
            occurrence_id.to_owned(),
            class,
            object_id.to_owned(),
            revision,
            digest.to_owned(),
        )))
    }

    /// One page can contribute at most `k` top rows, so when a page selects more rows than one batch its rows are judged in rank order, a batch at a time, and `top.admits` stops the page once it rejects the best unjudged row.
    /// Batches are sized to the walk's admission rate only while that rate describes the page: a batch that admits nothing shows it does not, so the rows the set still admits are judged in one more batch, as a page without batching would be.
    fn judge_selected(
        &mut self,
        kernel: &KernelStore,
        authority: Authority<'_>,
        Selected { candidates, scores }: Selected,
        budget: &EvalBudget,
    ) -> Result<ControlFlow<Option<AuthorityMoved>>, OracleRefusal> {
        let k = self.top.k();
        let total = candidates.len();
        if total == 0 {
            return Ok(ControlFlow::Continue(()));
        }
        if self.admissions.next_batch(k, k, total) == total {
            return self.judge_batch(kernel, authority, candidates, scores, budget);
        }
        let mut order: Vec<usize> = (0..total).collect();
        order.sort_unstable_by(|&left, &right| {
            rank_order(
                (scores[left], &candidates[left].occurrence_id),
                (scores[right], &candidates[right].occurrence_id),
            )
        });
        let mut slots: Vec<Option<OccurrenceCandidate>> =
            candidates.into_iter().map(Some).collect();
        let admits = |top: &TopK<OccurrenceCandidate>,
                      slots: &[Option<OccurrenceCandidate>],
                      index: usize| {
            let id = slots[index]
                .as_ref()
                .expect("unjudged")
                .occurrence_id
                .as_str();
            top.admits(scores[index], id)
        };
        let mut next = 0;
        let mut eligible_in_page = 0usize;
        let mut sized = true;
        while next < total {
            if !admits(&self.top, &slots, order[next]) {
                break;
            }
            let batch = if sized {
                self.admissions
                    .next_batch(k, k - eligible_in_page.min(k), total - next)
            } else {
                // Rank order makes the rows the set still admits a prefix of the unjudged rows.
                order[next..]
                    .iter()
                    .take_while(|&&index| admits(&self.top, &slots, index))
                    .count()
            };
            // Visit order is identifier order, which keeps the kernel's per-candidate registry probes local; the verdicts stay positional.
            let mut picked = order[next..next + batch].to_vec();
            picked.sort_unstable();
            let batch_candidates: Vec<OccurrenceCandidate> = picked
                .iter()
                .map(|&index| {
                    slots[index]
                        .take()
                        .expect("each selected row is judged at most once")
                })
                .collect();
            let batch_scores: Vec<f64> = picked.iter().map(|&index| scores[index]).collect();
            let before = self.admissions.eligible;
            let flow =
                self.judge_batch(kernel, authority, batch_candidates, batch_scores, budget)?;
            if flow.is_break() {
                return Ok(flow);
            }
            let eligible = self.admissions.eligible - before;
            sized &= eligible != 0;
            eligible_in_page += eligible;
            next += batch;
        }
        Ok(ControlFlow::Continue(()))
    }

    /// Accounts for `candidate` entering the held set before it moves in; `false` when the heap bound stops the walk instead.
    /// Only a ranked walk carries storage bounds, and it offers rows best first, so an admitted row never displaces a held one.
    fn hold(&mut self, candidate: &OccurrenceCandidate, score: f64) -> bool {
        let Some(storage) = self.stored.limits else {
            return true;
        };
        if !self.top.admits(score, &candidate.occurrence_id) {
            return true;
        }
        debug_assert!(
            !self.top.is_full(),
            "a ranked walk never displaces a held row"
        );
        let held = self.stored.held + held_bytes(candidate);
        let slots = self.top.k().saturating_mul(HELD_SLOT_BYTES);
        if slots.saturating_add(held) > storage.heap_bytes.get() {
            self.stored.stopped = Some(IncompleteReason::HeapBytes);
            return false;
        }
        self.stored.held = held;
        true
    }

    /// Judges one batch and offers its eligible rows to the top-K. A moved authority discards the batch's admissions; its exclusions still count as judged work.
    fn judge_batch(
        &mut self,
        kernel: &KernelStore,
        authority: Authority<'_>,
        candidates: Vec<OccurrenceCandidate>,
        scores: Vec<f64>,
        budget: &EvalBudget,
    ) -> Result<ControlFlow<Option<AuthorityMoved>>, OracleRefusal> {
        let Some((report, moved)) =
            judge_page(kernel, authority, &candidates, budget, &mut self.ranking)?
        else {
            return Ok(ControlFlow::Break(None));
        };
        (self.hook)(Window::AfterJudgment);
        // Exclusions are judged work whether the page is then scored, cancelled, or discarded for a moved authority.
        for judged in &report.occurrences {
            if let Disposition::PolicyExcluded(verdict) = judged.disposition {
                tally_exclusion(&mut self.ranking.consumed.excluded, verdict);
            }
        }
        if let Some(moved) = moved {
            // The moved batch's verdicts describe other facts, so none is admitted.
            return Ok(ControlFlow::Break(Some(moved)));
        }
        self.admissions.judged += candidates.len();
        for ((candidate, score), judged) in
            candidates.into_iter().zip(scores).zip(report.occurrences)
        {
            if budget.is_exhausted() {
                return Ok(ControlFlow::Break(None));
            }
            if judged.disposition == Disposition::Eligible {
                self.admissions.eligible += 1;
                if !self.hold(&candidate, score) {
                    return Ok(ControlFlow::Break(None));
                }
                let ranked = Ranked {
                    occurrence_id: candidate.occurrence_id.clone(),
                    class: candidate.class,
                    score,
                };
                self.top.offer(ranked, candidate);
            }
        }
        Ok(ControlFlow::Continue(()))
    }
}

/// Reads the seven page columns of the statement's current row by reference; a column whose stored type is not the schema's is a corrupt row.
fn borrow_row<'r>(row: &'r rusqlite::Row<'r>) -> Result<PageRow<'r>, OracleRefusal> {
    let column = |index: usize| row.get_ref(index).map_err(ProjectionError::from);
    let text = |index: usize| -> Result<&'r str, OracleRefusal> {
        Ok(column(index)?
            .as_str()
            .map_err(|_| ProjectionError::CorruptRow)?)
    };
    let integer = |index: usize| -> Result<i64, OracleRefusal> {
        Ok(column(index)?
            .as_i64()
            .map_err(|_| ProjectionError::CorruptRow)?)
    };
    let class = OccurrenceClass::from_code(text(1)?).ok_or(ProjectionError::CorruptRow)?;
    let stored = match column(5)? {
        rusqlite::types::ValueRef::Null => None,
        value => Some(value.as_blob().map_err(|_| ProjectionError::CorruptRow)?),
    };
    Ok(PageRow {
        occurrence_id: text(0)?,
        class,
        source_object_id: text(2)?,
        revision: integer(3)?,
        source_artifact_digest: text(4)?,
        stored,
        pending: match column(6)? {
            rusqlite::types::ValueRef::Null => None,
            _ => Some(integer(6)? != 0),
        },
    })
}

/// Judged and eligible counts over the walk so far; the ratio sizes judgment batches.
#[derive(Default)]
struct Admissions {
    judged: usize,
    eligible: usize,
}

impl Admissions {
    /// Sizes a ranked batch to the rows expected to yield the `wanted` eligible rows the set still lacks at the observed admission rate, `wanted` rows before any is judged and every remaining row while none has been eligible. The batch never exceeds `remaining`, so a walk whose rows are all eligible judges exactly `k`.
    fn ranked_batch(&self, wanted: usize, remaining: usize) -> usize {
        let want = match (self.judged, self.eligible) {
            (0, _) => wanted,
            (_, 0) => remaining,
            (judged, eligible) => wanted.saturating_mul(judged).div_ceil(eligible),
        };
        want.clamp(1, remaining)
    }

    /// Sizes a batch to the rows expected to yield `wanted` eligible rows at the walk's observed admission rate, never fewer than `k`: with every row eligible a batch is `k` rows and a page whose `k` best rows are all eligible is done after one batch, while a walk that admits almost nothing judges whole pages as it would without batching.
    /// A remainder of at most `k` rows is folded into the batch before it.
    fn next_batch(&self, k: usize, wanted: usize, remaining: usize) -> usize {
        let want = if self.judged == 0 {
            k
        } else {
            wanted
                .max(1)
                .saturating_mul(self.judged)
                .checked_div(self.eligible)
                .unwrap_or(usize::MAX)
        }
        .max(k);
        if remaining <= want.saturating_add(k) {
            remaining
        } else {
            want
        }
    }
}

/// The scored rows of a ranked walk still available to later batches, drawn best first under `order`, where `Less` ranks ahead.
/// `rows[..ranked]` is in rank order ahead of every later row, and `rows[..next]` is drawn. The first [`EXACT_PASSES`] extensions of the ranked prefix rank only the rows their draw needs; each later one at least doubles the prefix or exhausts the remaining rows, bounding a full drain to `EXACT_PASSES + log2 n` selection passes. Draws within the ranked prefix use zero comparator calls.
struct Unjudged<F> {
    rows: Vec<(f64, usize)>,
    next: usize,
    ranked: usize,
    passes: usize,
    order: F,
}

/// Selection passes that rank exactly the rows their draw needs before the ranked prefix starts doubling.
const EXACT_PASSES: usize = 3;

impl<F: Fn(&(f64, usize), &(f64, usize)) -> std::cmp::Ordering> Unjudged<F> {
    fn new(rows: Vec<(f64, usize)>, order: F) -> Self {
        Self {
            rows,
            next: 0,
            ranked: 0,
            passes: 0,
            order,
        }
    }

    fn len(&self) -> usize {
        self.rows.len() - self.next
    }

    /// Moves the best available rows onto the end of `drawn`, best first, until it holds `want` rows or the rows run out.
    fn draw(&mut self, drawn: &mut Vec<(f64, usize)>, want: usize) {
        let need = want.saturating_sub(drawn.len()).min(self.len());
        let missing = need.saturating_sub(self.ranked - self.next);
        if missing > 0 {
            let rest = &mut self.rows[self.ranked..];
            let grown = if self.passes < EXACT_PASSES {
                missing
            } else {
                missing.max(self.ranked)
            };
            let chunk = grown.min(rest.len());
            if chunk < rest.len() {
                rest.select_nth_unstable_by(chunk - 1, &self.order);
            }
            rest[..chunk].sort_unstable_by(&self.order);
            self.ranked += chunk;
            self.passes += 1;
        }
        drawn.extend_from_slice(&self.rows[self.next..self.next + need]);
        self.next += need;
    }
}

/// `None` means the budget ended during the judgment and `ranking` already says so.
fn judge_page(
    kernel: &KernelStore,
    authority: Authority<'_>,
    candidates: &[OccurrenceCandidate],
    budget: &EvalBudget,
    ranking: &mut ExhaustiveRanking,
) -> Result<Option<(EligibilityReport, Option<AuthorityMoved>)>, OracleRefusal> {
    let judged = match judge_tracked(
        kernel,
        authority,
        candidates,
        budget,
        &mut ranking.snapshot,
        &mut ranking.incarnation,
    ) {
        Ok(judged) => judged,
        Err(KernelError::Deadline) => {
            incomplete(ranking, IncompleteReason::BudgetExhausted);
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    ranking.consumed.batches += 1;
    ranking.consumed.judged += candidates.len();
    Ok(Some(judged))
}

fn moved_reason(moved: AuthorityMoved) -> IncompleteReason {
    match moved {
        AuthorityMoved::Incarnation => IncompleteReason::KernelIncarnationChanged,
        AuthorityMoved::Snapshot => IncompleteReason::SnapshotChanged,
    }
}

/// Re-judges the top-K in one batch and keeps only rows the kernel still admits, so a canonical change after admission cannot reach the caller.
fn revalidate(
    kernel: &KernelStore,
    authority: Authority<'_>,
    budget: &EvalBudget,
    top: TopK<OccurrenceCandidate>,
    ranking: &mut ExhaustiveRanking,
) -> Result<(), OracleRefusal> {
    if top.is_empty() {
        return Ok(());
    }
    if budget.is_exhausted() {
        incomplete(ranking, IncompleteReason::BudgetExhausted);
        return Ok(());
    }
    let (ranked, candidates): (Vec<Ranked>, Vec<OccurrenceCandidate>) =
        top.into_ranked().into_iter().unzip();
    let Some((report, moved)) = judge_page(kernel, authority, &candidates, budget, ranking)? else {
        return Ok(());
    };
    if let Some(moved) = moved {
        incomplete(ranking, moved_reason(moved));
    }
    ranking.snapshot = Some(report.snapshot);
    ranking.incarnation = Some(report.incarnation);
    for ((row, candidate), judged) in ranked.into_iter().zip(candidates).zip(report.occurrences) {
        match judged.disposition {
            Disposition::Eligible => {
                ranking.ranked.push(row);
                ranking.candidates.push(candidate);
            }
            Disposition::PolicyExcluded(verdict) => {
                tally_exclusion(&mut ranking.consumed.excluded, verdict);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn the_first_batch_of_a_walk_is_k_rows_and_a_short_remainder_folds_into_it() {
        let fresh = Admissions::default();
        assert_eq!(fresh.next_batch(8, 8, 256), 8);
        assert_eq!(fresh.next_batch(8, 8, 16), 16, "a remainder of k folds in");
        assert_eq!(fresh.next_batch(8, 8, 17), 8);
        assert_eq!(fresh.next_batch(8, 8, 5), 5);
    }

    #[test]
    fn later_batches_follow_the_observed_admission_rate() {
        let all_eligible = Admissions {
            judged: 64,
            eligible: 64,
        };
        assert_eq!(all_eligible.next_batch(8, 8, 256), 8);
        let one_in_eight = Admissions {
            judged: 8,
            eligible: 1,
        };
        assert_eq!(
            one_in_eight.next_batch(8, 7, 256),
            56,
            "seven more eligible rows at one in eight"
        );
        assert_eq!(
            one_in_eight.next_batch(8, 7, 60),
            60,
            "a remainder within k folds in"
        );
        let none_yet = Admissions {
            judged: 8,
            eligible: 0,
        };
        assert_eq!(
            none_yet.next_batch(8, 8, 248),
            248,
            "no eligible row seen: the rest of the page is one batch"
        );
        let mostly = Admissions {
            judged: 256,
            eligible: 200,
        };
        assert_eq!(mostly.next_batch(8, 8, 256), 10);
        assert_eq!(mostly.next_batch(8, 1, 256), 8, "never fewer than k");
    }

    #[test]
    fn a_ranked_batch_asks_for_the_rows_the_set_still_lacks() {
        let fresh = Admissions::default();
        assert_eq!(fresh.ranked_batch(8, 256), 8);
        assert_eq!(fresh.ranked_batch(8, 5), 5, "never past the remaining rows");
        let half = Admissions {
            judged: 8,
            eligible: 4,
        };
        assert_eq!(half.ranked_batch(4, 256), 8);
        assert_eq!(half.ranked_batch(3, 256), 6);
        let most = Admissions {
            judged: 10,
            eligible: 9,
        };
        assert_eq!(most.ranked_batch(1, 256), 2, "the quotient rounds up");
        let none = Admissions {
            judged: 8,
            eligible: 0,
        };
        assert_eq!(
            none.ranked_batch(8, 248),
            248,
            "no eligible row yet: every remaining row"
        );
    }

    /// A batch reserves no more slots than its batch bytes can fill, since every candidate it accepts costs at least [`SELECTED_ROW_BYTES`].
    #[test]
    fn a_batch_reserves_only_the_slots_its_batch_bytes_can_fill() {
        let stored = |batch_bytes: usize| Stored {
            limits: Some(StorageBounds {
                batch_bytes: NonZeroUsize::new(batch_bytes).unwrap(),
                heap_bytes: NonZeroUsize::new(1 << 20).unwrap(),
            }),
            ..Stored::default()
        };
        let one_row = SELECTED_ROW_BYTES + 40;
        assert_eq!(stored(one_row).batch_slots(1024), 1);
        assert_eq!(stored(3 * SELECTED_ROW_BYTES).batch_slots(1024), 3);
        assert_eq!(stored(SELECTED_ROW_BYTES - 1).batch_slots(1024), 0);
        assert_eq!(stored(1 << 30).batch_slots(1024), 1024, "never past `want`");
        assert_eq!(Stored::default().batch_slots(16), 16, "an unbounded walk");
        for batch_bytes in [1, one_row, 7 * SELECTED_ROW_BYTES + 3, 1 << 16] {
            assert!(stored(batch_bytes).batch_slots(1024) * SELECTED_ROW_BYTES <= batch_bytes);
        }
    }

    /// Drawing `n` rows one at a time costs at most `4 n log n` comparisons, so a scan that judges every row in small batches pays for one ranking of its rows.
    #[test]
    fn drawing_every_row_one_at_a_time_costs_n_log_n_comparisons() {
        let n = 1usize << 13;
        let rank = |left: &(f64, usize), right: &(f64, usize)| {
            right.0.total_cmp(&left.0).then(left.1.cmp(&right.1))
        };
        // Scores repeat, so ties fall to the key as `rank_order` falls to the identifier.
        let rows: Vec<(f64, usize)> = (0..n)
            .map(|key| (((key * 7919) % 509) as f64, key))
            .collect();
        let mut expected = rows.clone();
        expected.sort_by(rank);
        let comparisons = std::cell::Cell::new(0usize);
        let mut unjudged = Unjudged::new(rows, |left: &(f64, usize), right: &(f64, usize)| {
            comparisons.set(comparisons.get() + 1);
            rank(left, right)
        });
        let mut drawn = Vec::new();
        let mut order = Vec::with_capacity(n);
        while unjudged.len() != 0 {
            unjudged.draw(&mut drawn, 1);
            order.push(drawn.remove(0));
        }
        assert_eq!(order, expected);
        let bound = 4 * n * n.ilog2() as usize;
        assert!(
            comparisons.get() <= bound,
            "{} comparisons for {n} rows, above {bound}",
            comparisons.get()
        );
    }

    /// Rows a batch drew but did not judge stay ahead of every row still undrawn, so mixed batch sizes yield the full rank order.
    #[test]
    fn rows_left_drawn_stay_ahead_of_the_undrawn_rows() {
        let rank = |left: &(f64, usize), right: &(f64, usize)| {
            right.0.total_cmp(&left.0).then(left.1.cmp(&right.1))
        };
        let rows: Vec<(f64, usize)> = (0..200)
            .map(|key| (((key * 37) % 23) as f64, key))
            .collect();
        let mut expected = rows.clone();
        expected.sort_by(rank);
        let mut unjudged = Unjudged::new(rows, rank);
        let mut drawn = Vec::new();
        let mut order = Vec::new();
        for (want, taken) in [(5, 2), (1, 1), (7, 7), (3, 1), (9, 4), (2, 2)]
            .into_iter()
            .cycle()
        {
            if drawn.len() + unjudged.len() == 0 {
                break;
            }
            let want = want.min(drawn.len() + unjudged.len());
            unjudged.draw(&mut drawn, want);
            assert!(drawn.len() >= want);
            order.extend(drawn.drain(..taken.min(want)));
        }
        assert_eq!(order, expected);
    }

    /// A ranking discarded for an ended budget keeps `candidates` in step with the emptied `ranked`.
    #[test]
    fn an_exhausted_ranking_drops_its_candidates_with_its_rows() {
        let candidate = OccurrenceCandidate::new(
            "occ".to_string(),
            OccurrenceClass::Messages,
            "object".to_string(),
            1,
            "digest".to_string(),
        );
        let ranking = ExhaustiveRanking {
            ranked: vec![Ranked {
                occurrence_id: "occ".to_string(),
                class: OccurrenceClass::Messages,
                score: 1.0,
            }],
            candidates: vec![candidate],
            completion: Completion::Complete,
            coverage: DenseCoverage::default(),
            snapshot: None,
            incarnation: None,
            consumed: Consumed {
                pages: 1,
                ..Consumed::default()
            },
        };
        let ranking = exhausted(ranking).unwrap();
        assert!(ranking.ranked.is_empty());
        assert!(ranking.candidates.is_empty());
        assert_eq!(
            ranking.completion,
            Completion::Incomplete(IncompleteReason::BudgetExhausted)
        );
    }

    /// The walk must step the primary-key index in identifier order; a class-index search would sort the whole class on every page.
    #[test]
    fn the_page_query_steps_the_occurrence_identifier_index() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::BASELINE).unwrap();
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", *PAGE_SQL))
            .unwrap()
            .query_map(rusqlite::params!["gen", "", 3], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(|detail| {
                detail.contains(
                    "SEARCH o USING INDEX sqlite_autoindex_occurrences_1 (occurrence_id>?)",
                )
            }),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|detail| detail.contains("TEMP B-TREE")),
            "{plan:?}"
        );
    }

    #[test]
    fn each_page_query_serves_every_page_from_one_preparation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::BASELINE).unwrap();
        for sql in [&*PAGE_SQL, &*crate::dense::layered::LIVE_SQL] {
            for (after, limit) in [("", 3), ("b", 257), ("c", 2)] {
                let mut statement = conn.prepare_cached(sql).unwrap();
                {
                    let mut rows = statement
                        .query(rusqlite::params!["gen", after, limit])
                        .unwrap();
                    assert!(rows.next().unwrap().is_none());
                }
                assert_eq!(
                    statement.get_status(rusqlite::StatementStatus::RePrepare),
                    0,
                    "{sql}"
                );
            }
        }
    }
}
