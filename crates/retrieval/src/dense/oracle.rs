//! Walks every live occurrence whose class requires a vector in occurrence identifier order through bounded keyset pages, inside the caller's read transaction.
//! Each page is validated and scored as it is read; only the rows that would enter the current top-K are judged for canonical eligibility, in rank order and in batches sized so a page whose k best rows are eligible judges only those k, and the eligible ones are offered; the top-K is re-judged once before return.
//! Judging only rows that can enter the set returns the same rows as judging every row: a member of the final top-K outranks the worst held member at every earlier point of the walk, so it is never skipped.
//! A live required row without a vector is a coverage shortfall, so the result is incomplete even when every scored row was eligible; a kernel snapshot or incarnation that moves between batches ends the walk the same way.
//! The walk itself is shared: a `RowSource` supplies the page query and the validated score of each visited row, so the oracle reads `occurrence_vectors` and the layered ranking reads resolved layer rows through one judgment, admission, and revalidation path.

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
use super::score::{Ranked, TopK, rank_order, score_encoded};
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

/// Supplies the live required rows in identifier order and the vector each carries.
pub(super) trait RowSource {
    /// The page query, in the shape `Progress::visit_page` documents.
    fn page_sql(&self) -> &str;

    /// The score of a visited row's vector after it is validated against `layout`, or `None` when the source holds none for it.
    fn score(
        &mut self,
        row: &PageRow<'_>,
        layout: &RowLayout,
        query: &[f32],
    ) -> Result<Option<f64>, OracleRefusal>;

    /// Runs after every page whose rows were all visited, with whether rows remain past it.
    fn after_page(&mut self, _more: bool) {}
}

/// The projection's own `occurrence_vectors` of the generation.
struct StoredVectors;

impl RowSource for StoredVectors {
    fn page_sql(&self) -> &str {
        &PAGE_SQL
    }

    fn score(
        &mut self,
        row: &PageRow<'_>,
        layout: &RowLayout,
        query: &[f32],
    ) -> Result<Option<f64>, OracleRefusal> {
        row.stored
            .map(|bytes| {
                score_encoded(layout, query, bytes).map_err(|rejection| OracleRefusal::StoredRow {
                    occurrence_id: row.occurrence_id.to_owned(),
                    rejection,
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
    /// One or more per page that selected a row, plus the re-judgment; a page with no row that could enter the top-K runs none.
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
    /// Durable work for the row's vector is still open.
    pub pending: bool,
}

impl PageRow<'_> {
    fn candidate(&self) -> OccurrenceCandidate {
        OccurrenceCandidate::new(
            self.occurrence_id.to_owned(),
            self.class,
            self.source_object_id.to_owned(),
            self.revision,
            self.source_artifact_digest.to_owned(),
        )
    }
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
    hook: impl FnMut(Window<'_>),
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

    let mut progress = Progress {
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
        hook,
    };
    loop {
        if budget.is_exhausted() {
            return exhausted(progress.ranking);
        }
        let take = bounds
            .page_rows
            .get()
            .min(bounds.max_rows.get() - progress.ranking.coverage.required);
        let Some(page) = progress.visit_page(conn, request, source, take, budget)? else {
            return exhausted(progress.ranking);
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
                ControlFlow::Break(None) => return exhausted(progress.ranking),
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

/// The walk's state between pages: the result under construction, the held set, the admission counts that size judgment batches, the identifier to resume after, and the test hook.
struct Progress<H> {
    ranking: ExhaustiveRanking,
    top: TopK<OccurrenceCandidate>,
    admissions: Admissions,
    after: String,
    hook: H,
}

impl<H: FnMut(Window<'_>)> Progress<H> {
    /// Steps one page of at most `take` rows plus one probe row past it, scoring each row while the statement is positioned on it, so no column is copied for a row that cannot enter the top-K.
    /// The source's SQL selects the seven columns of [`PAGE_SQL`] in that order and binds the generation identifier, the identifier to start after, and the limit as `?1`, `?2`, `?3`; `after` is left at the last visited identifier.
    /// A row's identity fields are validated before `top.admits` is consulted, so a corrupt row is refused even when it could not enter the top-K; a missing vector is counted and its row is neither judged nor scored.
    /// `None` means the budget ended inside the page; `ranking` then describes the rows visited before it did.
    fn visit_page(
        &mut self,
        conn: &GuardedConn<'_>,
        request: &Walk<'_>,
        source: &mut impl RowSource,
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
        let mut selected: Option<Selected> = None;
        let mut visited = 0usize;
        loop {
            let row = match rows.next().map_err(ScanStop::from) {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(ScanStop::Budget) => return Ok(None),
                Err(ScanStop::Projection(error)) => return Err(error.into()),
            };
            if budget.check().is_err() {
                return Ok(None);
            }
            if visited == take {
                return Ok(Some(Page {
                    more: true,
                    selected,
                }));
            }
            let row = borrow_row(row)?;
            if selected.is_none() {
                self.ranking.consumed.pages += 1;
                selected = Some(Selected::default());
            }
            (self.hook)(Window::Visited(row.occurrence_id));
            if budget.is_exhausted() {
                return Ok(None);
            }
            visited += 1;
            self.after.clear();
            self.after.push_str(row.occurrence_id);
            self.ranking.coverage.required += 1;
            match source.score(&row, &request.layout, request.query)? {
                Some(score) => {
                    self.ranking.coverage.with_vector += 1;
                    EligibilityCandidate::validate_fields(
                        row.source_object_id,
                        Some(row.source_artifact_digest),
                    )?;
                    if self.top.admits(score, row.occurrence_id) {
                        let selected = selected.as_mut().expect("set at the first row");
                        selected.candidates.push(row.candidate());
                        selected.scores.push(score);
                    }
                }
                None if row.pending => self.ranking.coverage.missing_pending += 1,
                None => self.ranking.coverage.missing_without_pending += 1,
            }
        }
        Ok(Some(Page {
            more: false,
            selected,
        }))
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
        pending: integer(6)? != 0,
    })
}

/// Judged and eligible counts over the walk so far; the ratio sizes judgment batches.
#[derive(Default)]
struct Admissions {
    judged: usize,
    eligible: usize,
}

impl Admissions {
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
