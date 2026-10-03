//! Selects one global pool of quantized candidates over a composition's resolved layers.
//! Resolution runs first, so only each occurrence's winning row is scored; each winner scores its own layer's codes under a query encoded with that layer's scales.
//! The walk is the oracle's: a row enters judgment only when it could enter the pool, the kernel judges it in bounded batches before it is admitted, a rejected row takes no place in the pool, and the pool is re-judged once at the end.
//! A walk that ends before every live required row was visited, that reaches a storage bound, or whose authority moved returns no candidate; only a coverage shortfall leaves the pool in place, marked incomplete.

use std::num::NonZeroUsize;

use kernel::KernelStore;
use kernel::applicability::EvalBudget;
use storage::GuardedConn;

use super::capacity::CandidateCapacity;
use super::codec::{Metric, RowLayout};
use super::layered::{
    self, Cursor, LayerAccount, LayeredQuery, LayeredRefusal, Resolving, fault_refusal,
};
use super::oracle::{
    self, Completion, ExhaustiveRanking, IncompleteReason, Lane, OracleBounds, OracleRefusal,
    PageRow, RowSource, StorageBounds, Walk, Window,
};
use super::resolve::{Layer, RowFault};
use super::scalar::{self, QuantizedQuery, QueryRefusal, Scales};
use crate::batch::VectorGeneration;
use crate::eligibility::Authority;

/// A layer's int8 codes by row index, in the order of the layer's identifiers.
pub trait CodeAccess {
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
            RowFault::Unavailable(format!(
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
    #[error("layer {layer} holds {rows} rows but {codes} rows of codes")]
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

/// See [`super::oracle::exhaustive_with_hook_for_test`].
#[cfg(feature = "test-support")]
pub fn select_candidates_with_hook_for_test<'a>(
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
    for (layer, (rows, codes)) in request.layers.iter().zip(request.codes).enumerate() {
        let (rows, codes) = (rows.rows.row_count(), codes.codes.row_count());
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
