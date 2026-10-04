//! Selects one global pool of quantized candidates over a composition's resolved layers.
//! Resolution runs first, so only each occurrence's winning row is scored; each winner scores its own layer's codes under a query encoded with that layer's scales.
//! The walk is the oracle's: a row enters judgment only when it could enter the pool, the kernel judges it in bounded batches before it is admitted, a rejected row takes no place in the pool, and the pool is re-judged once at the end.
//! The pool is then rescored against each entry's original f32 row from the same layer, and the best `K` under the retained-f32 arithmetic are returned.
//! A walk that ends before every live required row was visited, that reaches a storage bound, or whose authority moved returns no candidate; only a coverage shortfall leaves the pool in place, marked incomplete.

use std::num::NonZeroUsize;

use kernel::KernelStore;
use kernel::applicability::EvalBudget;
use storage::GuardedConn;

use super::capacity::CandidateCapacity;
use super::codec::{self, Metric, RowLayout, RowRejection};
use super::layered::{
    self, Cursor, LayerAccount, LayeredQuery, LayeredRefusal, Resolving, fault_refusal,
};
use super::oracle::{
    self, Completion, ExhaustiveRanking, IncompleteReason, Lane, OracleBounds, OracleRefusal,
    PageRow, RowSource, StorageBounds, Walk, Window,
};
use super::resolve::{Layer, RowFault};
use super::scalar::{self, QuantizedQuery, QueryRefusal, Scales};
use super::score::{Ranked, rank_order, score_with_squares};
use crate::batch::VectorGeneration;
use crate::eligibility::{Authority, OccurrenceCandidate};

/// A layer's int8 codes by row index, in the order of the layer's identifiers.
pub trait CodeAccess {
    /// Rows of codes held; a scan refuses a layer whose identifiers number otherwise.
    fn row_count(&self) -> usize;

    /// Writes the codes of row `index` into `into`; the caller checks them against the recipe.
    fn codes_into(&self, index: usize, into: &mut Vec<i8>) -> Result<(), RowFault>;
}

/// Resident codes; a slice cannot stand behind `dyn`, so the owning vector is the implementor.
impl CodeAccess for Vec<Vec<i8>> {
    fn row_count(&self) -> usize {
        self.len()
    }

    fn codes_into(&self, index: usize, into: &mut Vec<i8>) -> Result<(), RowFault> {
        let codes = self.get(index).ok_or_else(|| {
            RowFault::Missing(format!(
                "row {index} is past the {} resident rows",
                self.len()
            ))
        })?;
        into.clear();
        into.extend_from_slice(codes);
        Ok(())
    }
}

/// One layer's codes and the scales they were encoded under.
pub struct LayerCodes<'a> {
    pub scales: &'a Scales,
    pub codes: &'a dyn CodeAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanBounds {
    /// Rows read and scored per page; at most the kernel's candidate batch.
    pub page_rows: NonZeroUsize,
    /// Live required rows one request may visit; a scan that reaches it returns no candidate.
    pub max_rows: NonZeroUsize,
    pub storage: StorageBounds,
}

/// Not `Debug`: the query row is embedding content.
pub struct CandidateQuery<'a> {
    pub generation: &'a VectorGeneration,
    pub metric: Metric,
    pub unit_norm_tolerance: f64,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub capacity: CandidateCapacity,
    pub bounds: ScanBounds,
    pub layers: &'a [Layer<'a>],
    /// One entry per layer, in the order of `layers`.
    pub codes: &'a [LayerCodes<'a>],
    /// Rows and tombstones the layers may carry together.
    pub max_entries: NonZeroUsize,
}

/// Where a pool entry's winning row sits: an index into the request's layers and a row of that layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinnerRow {
    pub layer: usize,
    pub row: usize,
}

/// The accepted pool, best quantized score first.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidatePool {
    /// `ranked` carries the quantized scores; `candidates` the terms each was judged under.
    pub ranking: ExhaustiveRanking,
    pub layers: LayerAccount,
    /// The winning layer row of each pool entry, in `ranking.ranked` order, so a rescore reads the original row of the same layer.
    pub winners: Vec<WinnerRow>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CandidateRefusal {
    /// The query encodes under every layer's scales before any row is read, whichever layers end up holding winners.
    #[error("the query cannot score layer {layer}'s codes: {refusal}")]
    Query { layer: usize, refusal: QueryRefusal },
    #[error("{codes} code sets were supplied for {layers} layers")]
    Codes { layers: usize, codes: usize },
    #[error("layer {layer} names {rows} occurrences but holds {codes} rows of codes")]
    CodeRows {
        layer: usize,
        rows: usize,
        codes: usize,
    },
    #[error(transparent)]
    Layered(#[from] LayeredRefusal),
}

/// What one visited winner carries into scoring: its layer and that layer's codes for it.
#[derive(Default)]
struct WinnerCodes {
    layer: usize,
    codes: Vec<i8>,
}

/// The int8 codes of each resolved winner, scored under the query encoded for its layer.
struct ResolvedCodes<'a> {
    codes: &'a [LayerCodes<'a>],
    queries: Vec<QuantizedQuery<'a>>,
    cursor: Cursor<'a>,
}

impl<'a> Resolving<'a> for ResolvedCodes<'a> {
    fn cursor(&self) -> &Cursor<'a> {
        &self.cursor
    }
}

impl RowSource for ResolvedCodes<'_> {
    type Payload = WinnerCodes;

    fn page_sql(&self) -> &str {
        &layered::LIVE_SQL
    }

    fn after_page(&mut self, more: bool) {
        self.cursor.after_page(more);
    }

    fn load(
        &mut self,
        row: &PageRow<'_>,
        layout: &RowLayout,
        into: &mut WinnerCodes,
    ) -> Result<bool, OracleRefusal> {
        let Some(winner) = self.cursor.seek(row.occurrence_id) else {
            return Ok(false);
        };
        self.codes[winner.layer]
            .codes
            .codes_into(winner.row, &mut into.codes)
            .map_err(|fault| fault_refusal(winner.occurrence_id, fault))?;
        scalar::check_codes(&into.codes, layout.dimension).map_err(|rejection| {
            OracleRefusal::Unreadable {
                occurrence_id: winner.occurrence_id.to_owned(),
                detail: rejection.to_string(),
            }
        })?;
        into.layer = winner.layer;
        Ok(true)
    }

    fn score(
        &self,
        _request: &Walk<'_>,
        lanes: &[Lane<WinnerCodes>],
        scores: &mut Vec<f64>,
    ) -> Result<(), OracleRefusal> {
        scores.clear();
        scores.extend(
            lanes
                .iter()
                .map(|lane| self.queries[lane.payload.layer].score(&lane.payload.codes)),
        );
        Ok(())
    }
}

/// # Errors
///
/// A code set per layer is required, and the query must encode under every layer's scales; both are checked before the projection is read. Every other refusal is the layered walk's.
pub fn select_candidates<'a>(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &CandidateQuery<'a>,
    budget: &EvalBudget,
) -> Result<CandidatePool, CandidateRefusal> {
    select_inner(conn, kernel, request, budget, |_| {})
}

/// [`select_candidates`] with `observe` run before each visited row is decoded, after each judgment and each page, and once before the final re-judgment, so a caller can watch the scan's progress or act inside its windows.
pub fn select_candidates_observed<'a>(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &CandidateQuery<'a>,
    budget: &EvalBudget,
    hook: impl FnMut(Window<'_>),
) -> Result<CandidatePool, CandidateRefusal> {
    select_inner(conn, kernel, request, budget, hook)
}

fn select_inner<'a>(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &CandidateQuery<'a>,
    budget: &EvalBudget,
    hook: impl FnMut(Window<'_>),
) -> Result<CandidatePool, CandidateRefusal> {
    if request.codes.len() != request.layers.len() {
        return Err(CandidateRefusal::Codes {
            layers: request.layers.len(),
            codes: request.codes.len(),
        });
    }
    for (layer, (named, codes)) in request.layers.iter().zip(request.codes).enumerate() {
        let (rows, codes) = (named.occurrence_ids.len(), codes.codes.row_count());
        if rows != codes {
            return Err(CandidateRefusal::CodeRows { layer, rows, codes });
        }
    }
    let layout = RowLayout {
        dimension: request.generation.vector_dimension,
        metric: request.metric,
        unit_norm_tolerance: request.unit_norm_tolerance,
    };
    let queries = request
        .codes
        .iter()
        .enumerate()
        .map(|(layer, codes)| {
            QuantizedQuery::new(&layout, codes.scales, request.query)
                .map_err(|refusal| CandidateRefusal::Query { layer, refusal })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let layered = LayeredQuery {
        generation: request.generation,
        metric: request.metric,
        unit_norm_tolerance: request.unit_norm_tolerance,
        query: request.query,
        authority: request.authority,
        bounds: OracleBounds {
            k: request.capacity.candidates(),
            page_rows: request.bounds.page_rows,
            max_rows: request.bounds.max_rows,
        },
        layers: request.layers,
        max_entries: request.max_entries,
    };
    let (mut ranked, source) = layered::walk_resolved(
        conn,
        kernel,
        &layered,
        Some(request.bounds.storage),
        budget,
        hook,
        |cursor| ResolvedCodes {
            codes: request.codes,
            queries,
            cursor,
        },
    )?;
    let ranking = &mut ranked.ranking;
    // A discarded pool keeps its counts and carries no authority stamp a caller could mistake for a usable one.
    if let Completion::Incomplete(reason) = ranking.completion
        && reason != IncompleteReason::DenseCoverageShortfall
    {
        oracle::drop_rows(ranking);
        ranking.snapshot = None;
        ranking.incarnation = None;
    }
    // The cursor's winners are in identifier order, and every pool entry is one of them.
    let winners = &source.cursor().winners;
    let winners = ranking
        .ranked
        .iter()
        .map(|row| {
            let index = winners
                .binary_search_by(|winner| {
                    winner
                        .occurrence_id
                        .as_bytes()
                        .cmp(row.occurrence_id.as_bytes())
                })
                .expect("every pool entry is a resolved winner");
            WinnerRow {
                layer: winners[index].layer,
                row: winners[index].row,
            }
        })
        .collect();
    Ok(CandidatePool {
        ranking: ranked.ranking,
        layers: ranked.layers,
        winners,
    })
}

/// The pool rescored against its original rows: at most `k` entries, best first, with the terms each was judged under.
#[derive(Debug, Clone, PartialEq)]
pub struct Rescored {
    pub ranked: Vec<Ranked>,
    pub candidates: Vec<OccurrenceCandidate>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RescoreRefusal<E> {
    #[error("the query is not a member of the generation: {0}")]
    Query(RowRejection),
    #[error("the original row of occurrence {occurrence_id} could not be read: {fault}")]
    Read {
        occurrence_id: String,
        winner: WinnerRow,
        fault: E,
    },
    #[error(
        "the original row of occurrence {occurrence_id} is not a member of the generation: {rejection}"
    )]
    Row {
        occurrence_id: String,
        winner: WinnerRow,
        rejection: RowRejection,
    },
}

/// Reads the original row of every pool entry through `read`, in pool order and once each, scores it against `query` with the retained-f32 arithmetic, and returns the `k` best under the dense order.
/// `read` replaces the contents of one row buffer reused across entries.
/// Only pool entries are read, and each from the winning layer row the pool names, so the result is `Top(k, A, f32_score)` over the accepted set `A`.
pub fn rescore_pool<E>(
    pool: &CandidatePool,
    layout: &RowLayout,
    query: &[f32],
    k: NonZeroUsize,
    mut read: impl FnMut(&WinnerRow, &mut Vec<f32>) -> Result<(), E>,
) -> Result<Rescored, RescoreRefusal<E>> {
    codec::validate(query, layout).map_err(RescoreRefusal::Query)?;
    let ranking = &pool.ranking;
    let mut scored = Vec::with_capacity(pool.winners.len());
    let mut row = Vec::with_capacity(query.len());
    for (index, (winner, candidate)) in pool.winners.iter().zip(&ranking.candidates).enumerate() {
        let occurrence_id = &candidate.occurrence_id;
        read(winner, &mut row).map_err(|fault| RescoreRefusal::Read {
            occurrence_id: occurrence_id.clone(),
            winner: *winner,
            fault,
        })?;
        let reject = |rejection| RescoreRefusal::Row {
            occurrence_id: occurrence_id.clone(),
            winner: *winner,
            rejection,
        };
        // The length is checked before the pass that scores and validates the row.
        codec::validate_length(&row, layout.dimension).map_err(reject)?;
        let sums = score_with_squares(layout.metric, query, &row);
        codec::validate_from_sum(&row, layout, sums.sum_of_squares).map_err(reject)?;
        scored.push((sums.score, index));
    }
    scored.sort_by(|left, right| {
        rank_order(
            (left.0, &ranking.candidates[left.1].occurrence_id),
            (right.0, &ranking.candidates[right.1].occurrence_id),
        )
    });
    scored.truncate(k.get());
    let candidates: Vec<OccurrenceCandidate> = scored
        .iter()
        .map(|(_, index)| ranking.candidates[*index].clone())
        .collect();
    let ranked = scored
        .into_iter()
        .zip(&candidates)
        .map(|((score, _), candidate)| Ranked {
            occurrence_id: candidate.occurrence_id.clone(),
            class: candidate.class,
            score,
        })
        .collect();
    Ok(Rescored { ranked, candidates })
}
