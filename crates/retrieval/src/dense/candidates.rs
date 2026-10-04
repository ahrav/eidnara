//! Selects one global pool of quantized candidates over a composition's resolved layers.
//! Resolution runs first, so only each occurrence's winning row is scored; each winner scores its own layer's codes under a query encoded with that layer's scales.
//! The oracle's ranked walk visits the live population in rowid order and marks the live winners, every live winner is scored, and the kernel judges them best first in bounded batches before any is admitted, until the pool holds `R` eligible rows or the winners run out.
//! A rejected row takes no place in the pool and never ends the scan; every batch runs after the walk under one snapshot, so the pool needs no re-judgment, and a pool that fills was judged over itself, every excluded row ranked above its last member, and the rest of the batch that filled it.
//! Beyond the pool and one judgment batch, the scan keeps per resolved winner a live bit, a rowid, and a slot of a prefix table at most half full, and per scored row a score and a winner index; the resolution's `max_entries` bounds them all.
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
    self, Completion, ExhaustiveRanking, IncompleteReason, OracleBounds, OracleRefusal,
    RankedSource, StorageBounds, Walk, Window,
};
use super::resolve::{Layer, RowFault};
use super::scalar::{self, QuantizedQuery, QueryRefusal, Scales};
use super::score::{BLOCK_ROWS, Ranked, rank_order, score_with_squares};
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
    /// Rows read per page, and the most rows one judgment batch holds; at most the kernel's candidate batch.
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

/// The int8 codes of each resolved winner, scored under the query encoded for its layer once the walk has marked which winners are live.
struct ResolvedCodes<'a> {
    codes: &'a [LayerCodes<'a>],
    queries: Vec<QuantizedQuery<'a>>,
    dimension: u32,
    storage: StorageBounds,
    cursor: Cursor<'a>,
    /// One bit per winner, set when the walk visits its row.
    live: Vec<u64>,
    /// The rowid each live winner's row was visited at, so judgment reads the row again without an identifier search.
    rowids: Vec<i64>,
}

impl<'a> Resolving<'a> for ResolvedCodes<'a> {
    fn cursor(&self) -> &Cursor<'a> {
        &self.cursor
    }

    fn walk(
        &mut self,
        conn: &GuardedConn<'_>,
        kernel: &KernelStore,
        request: &Walk<'_>,
        budget: &EvalBudget,
        hook: impl FnMut(Window<'_>),
    ) -> Result<ExhaustiveRanking, OracleRefusal> {
        let storage = self.storage;
        oracle::walk_ranked(conn, kernel, request, storage, budget, self, hook)
    }
}

impl RankedSource for ResolvedCodes<'_> {
    fn page_sql(&self) -> &str {
        &layered::LIVE_ROWID_SQL
    }

    fn visit(&mut self, occurrence_id: &str, rowid: i64) -> bool {
        let Some(index) = self.cursor.find(occurrence_id) else {
            return false;
        };
        self.live[index / 64] |= 1 << (index % 64);
        self.rowids[index] = rowid;
        true
    }

    fn after_page(&mut self, more: bool) {
        self.cursor.after_page(more);
    }

    /// Scores the live winners layer by layer in identifier order, eight rows of one layer at a time; a short final block of a layer scores row by row, with bit-identical results.
    fn score(
        &mut self,
        budget: &EvalBudget,
        scored: &mut Vec<(f64, usize)>,
    ) -> Result<bool, OracleRefusal> {
        let mut blocks: Vec<Block> = self.codes.iter().map(|_| Block::default()).collect();
        for (word_index, word) in self.live.iter().enumerate() {
            let mut word = *word;
            while word != 0 {
                let index = word_index * 64 + word.trailing_zeros() as usize;
                word &= word - 1;
                let winner = &self.cursor.winners[index];
                let block = &mut blocks[winner.layer];
                let lane = &mut block.lanes[block.filled];
                self.codes[winner.layer]
                    .codes
                    .codes_into(winner.row, &mut lane.codes)
                    .map_err(|fault| fault_refusal(winner.occurrence_id, fault))?;
                scalar::check_codes(&lane.codes, self.dimension).map_err(|rejection| {
                    OracleRefusal::Unreadable {
                        occurrence_id: winner.occurrence_id.to_owned(),
                        detail: rejection.to_string(),
                    }
                })?;
                lane.winner = index;
                block.filled += 1;
                if block.filled == BLOCK_ROWS {
                    if budget.is_exhausted() {
                        return Ok(false);
                    }
                    block.score(&self.queries[winner.layer], scored);
                }
            }
        }
        for (layer, block) in blocks.iter_mut().enumerate() {
            block.score(&self.queries[layer], scored);
        }
        Ok(true)
    }

    fn occurrence_id(&self, key: usize) -> &str {
        self.cursor.winners[key].occurrence_id
    }

    fn rowid(&self, key: usize) -> i64 {
        self.rowids[key]
    }
}

/// One live winner's codes, copied out of its layer.
#[derive(Default)]
struct Lane {
    winner: usize,
    codes: Vec<i8>,
}

/// Up to [`BLOCK_ROWS`] live winners of one layer.
#[derive(Default)]
struct Block {
    lanes: [Lane; BLOCK_ROWS],
    filled: usize,
}

impl Block {
    /// Scores the filled lanes into `scored` and empties the block; block scoring is bit-for-bit identical to scoring each row alone.
    fn score(&mut self, query: &QuantizedQuery<'_>, scored: &mut Vec<(f64, usize)>) {
        let filled = std::mem::take(&mut self.filled);
        match filled {
            0 => {}
            1 => scored.push((query.score(&self.lanes[0].codes), self.lanes[0].winner)),
            _ => {
                let rows = std::array::from_fn(|slot| {
                    self.lanes[if slot < filled { slot } else { 0 }]
                        .codes
                        .as_slice()
                });
                let scores = query.score_block(&rows);
                scored.extend(
                    scores
                        .into_iter()
                        .zip(self.lanes[..filled].iter().map(|lane| lane.winner)),
                );
            }
        }
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

/// `hook` runs before each visited row is read further, after every page, and after every judgment batch, so a test can change the kernel or the budget in those windows.
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
    let (mut ranked, source) =
        layered::walk_resolved(conn, kernel, &layered, budget, hook, |cursor| {
            let winners = cursor.winners.len();
            ResolvedCodes {
                codes: request.codes,
                queries,
                dimension: layout.dimension,
                storage: request.bounds.storage,
                cursor,
                live: vec![0; winners.div_ceil(64)],
                rowids: vec![0; winners],
            }
        })?;
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
