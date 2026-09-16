//! Walks every live occurrence whose class requires a vector in occurrence identifier order through bounded keyset pages, inside the caller's read transaction.
//! Each page is decoded, validated, and scored in blocks of eight rows; only the rows that would enter the current top-K are judged for canonical eligibility, in one kernel batch per page, and the eligible ones are offered; the top-K is re-judged once before return.
//! Judging only rows that can enter the set returns the same rows as judging every row: a member of the final top-K outranks the worst held member at every earlier point of the walk, so it is never skipped.
//! A live required row without a vector is a coverage shortfall, so the result is incomplete even when every scored row was eligible; a kernel snapshot or incarnation that moves between batches ends the walk the same way.
//! The walk itself is shared: a `RowSource` supplies the page query and the vector of each visited row, so the oracle reads `occurrence_vectors` and the layered ranking reads resolved layer rows through one judgment, admission, and revalidation path.

use std::num::NonZeroUsize;
use std::ops::ControlFlow;
use std::sync::LazyLock;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CommitReadIncarnation, EgressSnapshot, EligibilityVerdict, KernelError, KernelStore,
    MAX_ELIGIBILITY_CANDIDATES,
};
use rusqlite::params;
use storage::GuardedConn;

use super::codec::{self, Metric, RowLayout, RowRejection};
use super::score::{BLOCK_ROWS, Ranked, TopK, score_block};
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
    /// Rows read, decoded, and scored per page; at most [`MAX_ELIGIBILITY_CANDIDATES`] so the rows a page selects for judgment fit one batch.
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

/// Supplies the live required rows in identifier order and the vector each carries.
pub(super) trait RowSource {
    /// The page query, in the shape [`read_page`] documents.
    fn page_sql(&self) -> &str;

    /// Returns a vector of the layout's length, or `None` when the source holds none for the row. Block scoring validates finiteness and norm.
    fn vector(
        &mut self,
        row: &PageRow,
        layout: &RowLayout,
    ) -> Result<Option<Vec<f32>>, OracleRefusal>;

    /// Runs after every page whose rows were all visited, with whether rows remain past it.
    fn after_page(&mut self, _more: bool) {}
}

/// The projection's own `occurrence_vectors` of the generation.
struct StoredVectors;

impl RowSource for StoredVectors {
    fn page_sql(&self) -> &str {
        &PAGE_SQL
    }

    fn vector(
        &mut self,
        row: &PageRow,
        layout: &RowLayout,
    ) -> Result<Option<Vec<f32>>, OracleRefusal> {
        row.stored
            .as_deref()
            .map(|bytes| {
                codec::decode_length(bytes, layout.dimension).map_err(|rejection| {
                    OracleRefusal::StoredRow {
                        occurrence_id: row.candidate.occurrence_id.clone(),
                        rejection,
                    }
                })
            })
            .transpose()
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
    /// Rows the kernel judged: those that could have entered the top-K when visited, plus the final re-judgment.
    pub judged: usize,
    /// At most one per page plus the re-judgment; a page with no row that could enter the top-K runs none.
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
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window<'a> {
    /// A live required row is about to be read; its block is validated and scored after the visits of its rows.
    Visited(&'a str),
    AfterJudgment,
    AfterPage(usize),
    BeforeRevalidation,
}

pub(super) struct PageRow {
    pub candidate: OccurrenceCandidate,
    /// The projection's stored vector bytes, when the page query selects them.
    pub stored: Option<Vec<u8>>,
    /// Durable work for the row's vector is still open.
    pub pending: bool,
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
static PAGE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest,v.vector,
                CASE WHEN v.vector IS NULL THEN {CURRENT_PENDING} ELSE 0 END
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         LEFT JOIN occurrence_vectors v ON v.occurrence_id=o.occurrence_id AND v.generation_id=?1
         WHERE t.occurrence_id IS NULL AND +o.class IN ({}) AND o.occurrence_id>?2
         ORDER BY o.occurrence_id
         LIMIT ?3",
        *DENSE_CLASSES
    )
});

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

/// Walks `source`'s live required rows under `request`'s bounds; see [`exhaustive`] for the result's meaning.
pub(super) fn walk(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &Walk<'_>,
    budget: &EvalBudget,
    source: &mut impl RowSource,
    mut hook: impl FnMut(Window<'_>),
) -> Result<ExhaustiveRanking, OracleRefusal> {
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
    crate::vectors::check_generation(conn, request.generation)?;

    let mut ranking = ExhaustiveRanking {
        ranked: Vec::new(),
        candidates: Vec::new(),
        completion: Completion::Complete,
        coverage: DenseCoverage::default(),
        snapshot: None,
        incarnation: None,
        consumed: Consumed::default(),
    };
    let mut top: TopK<OccurrenceCandidate> = TopK::new(bounds.k);
    let mut after = String::new();
    loop {
        if budget.is_exhausted() {
            return exhausted(ranking);
        }
        let take = bounds
            .page_rows
            .get()
            .min(bounds.max_rows.get() - ranking.coverage.required);
        let (page, more) = match read_page(
            conn,
            source.page_sql(),
            &request.generation.generation_id,
            &after,
            take,
            budget,
        ) {
            Ok(read) => read,
            Err(ScanStop::Budget) => return exhausted(ranking),
            Err(ScanStop::Projection(error)) => return Err(error.into()),
        };
        if let Some(last) = page.last() {
            after.clone_from(&last.candidate.occurrence_id);
            let Some(selected) =
                score_page(page, request, source, budget, &mut ranking, &top, &mut hook)?
            else {
                return exhausted(ranking);
            };
            source.after_page(more);
            let flow = judge_selected(
                kernel,
                request.authority,
                selected,
                budget,
                &mut ranking,
                &mut top,
                &mut hook,
            )?;
            hook(Window::AfterPage(ranking.consumed.pages));
            match flow {
                ControlFlow::Break(Some(moved)) => {
                    incomplete(&mut ranking, moved_reason(moved));
                    break;
                }
                ControlFlow::Break(None) => return exhausted(ranking),
                ControlFlow::Continue(()) => {}
            }
        } else {
            source.after_page(more);
        }
        if !more {
            break;
        }
        if ranking.coverage.required == bounds.max_rows.get() {
            incomplete(&mut ranking, IncompleteReason::RowBound);
            break;
        }
    }
    hook(Window::BeforeRevalidation);
    revalidate(kernel, request.authority, budget, top, &mut ranking)?;
    if budget.is_exhausted() {
        return exhausted(ranking);
    }
    // A walk that stopped early already names the stronger reason; a shortfall is only the reason when the walk otherwise completed.
    if ranking.coverage.missing() != 0 {
        incomplete(&mut ranking, IncompleteReason::DenseCoverageShortfall);
    }
    Ok(ranking)
}

/// Rows admitted before the budget ended were never re-judged under it, so none is returned.
fn exhausted(mut ranking: ExhaustiveRanking) -> Result<ExhaustiveRanking, OracleRefusal> {
    if ranking.consumed.pages == 0 {
        return Err(OracleRefusal::BudgetExhausted);
    }
    ranking.ranked.clear();
    ranking.candidates.clear();
    incomplete(&mut ranking, IncompleteReason::BudgetExhausted);
    Ok(ranking)
}

/// The first reason stands, except that an ended budget always wins: whatever was known before, the result was not finished.
fn incomplete(ranking: &mut ExhaustiveRanking, reason: IncompleteReason) {
    if reason == IncompleteReason::BudgetExhausted || ranking.completion == Completion::Complete {
        ranking.completion = Completion::Incomplete(reason);
    }
}

/// Reads one row past `take` to learn whether rows remain without retaining the extra row; `take == 0` is a pure remainder probe.
/// `sql` selects the seven columns of [`PAGE_SQL`] in that order and binds the generation identifier, the identifier to start after, and the limit as `?1`, `?2`, `?3`.
fn read_page(
    conn: &GuardedConn<'_>,
    sql: &str,
    generation_id: &str,
    after: &str,
    take: usize,
    budget: &EvalBudget,
) -> Result<(Vec<PageRow>, bool), ScanStop> {
    let limit = i64::try_from(take.saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(sql)?;
    let mut rows = statement.query(params![generation_id, after, limit])?;
    let mut page = Vec::with_capacity(take);
    while let Some(row) = rows.next()? {
        budget.check().map_err(|_| ScanStop::Budget)?;
        if page.len() == take {
            return Ok((page, true));
        }
        let class = OccurrenceClass::from_code(&row.get::<_, String>(1)?)
            .ok_or(ProjectionError::CorruptRow)?;
        page.push(PageRow {
            candidate: OccurrenceCandidate::new(
                row.get(0)?,
                class,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ),
            stored: row.get(5)?,
            pending: row.get(6)?,
        });
    }
    Ok((page, false))
}

/// Rows of one page that could enter the top-K, with their scores in the same order.
#[derive(Default)]
struct Selected {
    candidates: Vec<OccurrenceCandidate>,
    scores: Vec<f64>,
}

struct Block {
    rows: Vec<(OccurrenceCandidate, Vec<f32>)>,
}

impl Block {
    fn new() -> Self {
        Self {
            rows: Vec::with_capacity(BLOCK_ROWS),
        }
    }

    fn push(&mut self, candidate: OccurrenceCandidate, vector: Vec<f32>) {
        self.rows.push((candidate, vector));
    }

    fn is_full(&self) -> bool {
        self.rows.len() == BLOCK_ROWS
    }

    /// Validates and scores the buffered rows in visit order, exactly as scoring each row alone would: the first row that fails the layout refuses, every validated row counts toward coverage, and only then is its identity validated and its score offered to `top`.
    /// A block short of `BLOCK_ROWS` repeats its first row in the spare lanes; their results are discarded.
    fn flush(
        &mut self,
        request: &Walk<'_>,
        ranking: &mut ExhaustiveRanking,
        top: &TopK<OccurrenceCandidate>,
        selected: &mut Selected,
    ) -> Result<(), OracleRefusal> {
        let Some((_, first)) = self.rows.first() else {
            return Ok(());
        };
        let lanes: [&[f32]; BLOCK_ROWS] =
            std::array::from_fn(|lane| self.rows.get(lane).map_or(&first[..], |(_, row)| row));
        let sums = score_block(request.layout.metric, request.query, &lanes);
        for ((candidate, vector), (sum_of_squares, score)) in self
            .rows
            .drain(..)
            .zip(sums.sums_of_squares.into_iter().zip(sums.scores))
        {
            codec::validate_from_sum(&vector, &request.layout, sum_of_squares).map_err(
                |rejection| OracleRefusal::StoredRow {
                    occurrence_id: candidate.occurrence_id.clone(),
                    rejection,
                },
            )?;
            ranking.coverage.with_vector += 1;
            candidate.candidate.validate()?;
            if top.admits(score, &candidate.occurrence_id) {
                selected.candidates.push(candidate);
                selected.scores.push(score);
            }
        }
        Ok(())
    }
}

/// Every present vector of the page is obtained and, in blocks of up to [`BLOCK_ROWS`], validated and scored; a missing vector is counted and its row is neither judged nor scored.
/// A row with a vector has its identity fields validated before `top.admits` is consulted, so a corrupt row is refused even when it could not enter the top-K.
/// Returns the rows that would enter the top-K as it stood before the page, with their scores; only they are judged.
fn score_page(
    page: Vec<PageRow>,
    request: &Walk<'_>,
    source: &mut impl RowSource,
    budget: &EvalBudget,
    ranking: &mut ExhaustiveRanking,
    top: &TopK<OccurrenceCandidate>,
    hook: &mut impl FnMut(Window<'_>),
) -> Result<Option<Selected>, OracleRefusal> {
    ranking.consumed.pages += 1;
    let mut selected = Selected::default();
    let mut block = Block::new();
    for row in page {
        hook(Window::Visited(&row.candidate.occurrence_id));
        if budget.is_exhausted() {
            // Rows visited before the budget ended are judged for the layout before the end is acted on, so a corrupt row among them still refuses.
            block.flush(request, ranking, top, &mut selected)?;
            return Ok(None);
        }
        ranking.coverage.required += 1;
        match source.vector(&row, &request.layout)? {
            Some(vector) => {
                block.push(row.candidate, vector);
                if block.is_full() {
                    block.flush(request, ranking, top, &mut selected)?;
                }
            }
            None if row.pending => ranking.coverage.missing_pending += 1,
            None => ranking.coverage.missing_without_pending += 1,
        }
    }
    block.flush(request, ranking, top, &mut selected)?;
    Ok(Some(selected))
}

/// Judges the page's selected rows in one batch and offers the eligible ones to the top-K.
/// A moved authority discards the page's verdicts and stops the walk with its reason; a budget that ends inside the judgment stops it with none, the reason already recorded.
fn judge_selected(
    kernel: &KernelStore,
    authority: Authority<'_>,
    Selected { candidates, scores }: Selected,
    budget: &EvalBudget,
    ranking: &mut ExhaustiveRanking,
    top: &mut TopK<OccurrenceCandidate>,
    hook: &mut impl FnMut(Window<'_>),
) -> Result<ControlFlow<Option<AuthorityMoved>>, OracleRefusal> {
    if candidates.is_empty() {
        return Ok(ControlFlow::Continue(()));
    }
    let Some((report, moved)) = judge_page(kernel, authority, &candidates, budget, ranking)? else {
        return Ok(ControlFlow::Break(None));
    };
    hook(Window::AfterJudgment);
    // Exclusions are judged work whether the page is then scored, cancelled, or discarded for a moved authority.
    for judged in &report.occurrences {
        if let Disposition::PolicyExcluded(verdict) = judged.disposition {
            tally_exclusion(&mut ranking.consumed.excluded, verdict);
        }
    }
    if let Some(moved) = moved {
        // The moved batch's verdicts describe other facts, so none is admitted.
        return Ok(ControlFlow::Break(Some(moved)));
    }
    for ((candidate, score), judged) in candidates.into_iter().zip(scores).zip(report.occurrences) {
        if budget.is_exhausted() {
            return Ok(ControlFlow::Break(None));
        }
        if judged.disposition == Disposition::Eligible {
            let ranked = Ranked {
                occurrence_id: candidate.occurrence_id.clone(),
                class: candidate.class,
                score,
            };
            top.offer(ranked, candidate);
        }
    }
    Ok(ControlFlow::Continue(()))
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
}
