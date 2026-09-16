//! Ranks the live required occurrences of the projection against the rows a composition's layers resolve to.
//! Resolution runs first and alone: the winners are fixed before any row is judged, so neither enumeration order nor score can choose among layers.
//! The walk is the oracle's: live required rows in identifier order, judged in bounded batches, admitted to a bounded top-K, and re-judged once at the end.
//! A winner the projection no longer lists as live is revoked and never scored; no older row of its occurrence stands in for it.
//! A live required row no layer holds is a coverage shortfall, as in the oracle.

use std::num::NonZeroUsize;
use std::sync::LazyLock;

use kernel::KernelStore;
use kernel::applicability::EvalBudget;
use storage::GuardedConn;

use super::codec::{self, Metric, RowLayout};
use super::oracle::{
    self, ExhaustiveRanking, OracleBounds, OracleRefusal, PageRow, RowSource, Walk, Window,
};
use super::resolve::{self, Layer, ResolveRefusal, RowFault, Winner};
use crate::batch::VectorGeneration;
use crate::coverage::CURRENT_PENDING;
use crate::eligibility::Authority;

/// Not `Debug`: the query row is embedding content.
pub struct LayeredQuery<'a> {
    pub generation: &'a VectorGeneration,
    pub metric: Metric,
    pub unit_norm_tolerance: f64,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub bounds: OracleBounds,
    pub layers: &'a [Layer<'a>],
    /// Rows and tombstones the layers may carry together.
    pub max_entries: NonZeroUsize,
}

/// What became of the layers' rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerAccount {
    pub winners: usize,
    pub superseded: usize,
    pub masked: usize,
    /// Winners the projection no longer lists as live; never scored and never replaced by an older row.
    pub revoked: usize,
    /// Winners past the last row of a walk that ended before the population did; whether they are live is unknown.
    pub unvisited: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayeredRanking {
    pub ranking: ExhaustiveRanking,
    pub layers: LayerAccount,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LayeredRefusal {
    #[error("the layers do not resolve: {0}")]
    Resolve(#[from] ResolveRefusal),
    #[error("the layers carry base epoch {layers}, not the generation's {generation}")]
    Epoch { layers: u64, generation: u64 },
    #[error(transparent)]
    Oracle(#[from] OracleRefusal),
}

/// The oracle's page over live required rows without the stored vector; the walk takes vectors from the resolved layers instead.
static LIVE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest,NULL,
                {CURRENT_PENDING}
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE t.occurrence_id IS NULL AND +o.class IN ({}) AND o.occurrence_id>?2
         ORDER BY o.occurrence_id
         LIMIT ?3",
        *oracle::DENSE_CLASSES
    )
});

/// Merges the winners, in identifier order, with the walk's rows in the same order.
struct ResolvedRows<'a> {
    layers: &'a [Layer<'a>],
    winners: Vec<Winner<'a>>,
    next: usize,
    revoked: usize,
    /// The last page read had no rows after it, so every winner still ahead of the cursor is past the live population.
    exhausted: bool,
}

impl RowSource for ResolvedRows<'_> {
    fn page_sql(&self) -> &str {
        &LIVE_SQL
    }

    fn after_page(&mut self, more: bool) {
        self.exhausted = !more;
    }

    /// Winners before the visited row are not live any more; the winner at it is its vector; a winner after it waits.
    fn vector(
        &mut self,
        row: &PageRow,
        layout: &RowLayout,
    ) -> Result<Option<Vec<f32>>, OracleRefusal> {
        let visited = row.candidate.occurrence_id.as_bytes();
        while let Some(winner) = self.winners.get(self.next) {
            match winner.occurrence_id.as_bytes().cmp(visited) {
                std::cmp::Ordering::Less => {
                    self.revoked += 1;
                    self.next += 1;
                }
                std::cmp::Ordering::Equal => {
                    self.next += 1;
                    let occurrence_id = winner.occurrence_id;
                    let vector = match self.layers[winner.layer].rows.row(winner.row) {
                        Ok(vector) => vector,
                        Err(RowFault::Rejected(rejection)) => {
                            return Err(OracleRefusal::StoredRow {
                                occurrence_id: occurrence_id.to_owned(),
                                rejection,
                            });
                        }
                        Err(RowFault::Unavailable(detail)) => {
                            return Err(OracleRefusal::Unreadable {
                                occurrence_id: occurrence_id.to_owned(),
                                detail,
                            });
                        }
                    };
                    codec::validate(&vector, layout).map_err(|rejection| {
                        OracleRefusal::StoredRow {
                            occurrence_id: occurrence_id.to_owned(),
                            rejection,
                        }
                    })?;
                    return Ok(Some(vector));
                }
                std::cmp::Ordering::Greater => return Ok(None),
            }
        }
        Ok(None)
    }
}

/// # Errors
///
/// A layer set that does not resolve refuses before the projection is read; every other refusal is the oracle's.
pub fn rank_layers(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &LayeredQuery<'_>,
    budget: &EvalBudget,
) -> Result<LayeredRanking, LayeredRefusal> {
    rank_layers_inner(conn, kernel, request, budget, |_| {})
}

/// See [`oracle::exhaustive_with_hook_for_test`].
#[cfg(feature = "test-support")]
pub fn rank_layers_with_hook_for_test(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &LayeredQuery<'_>,
    budget: &EvalBudget,
    hook: impl FnMut(Window<'_>),
) -> Result<LayeredRanking, LayeredRefusal> {
    rank_layers_inner(conn, kernel, request, budget, hook)
}

fn rank_layers_inner(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &LayeredQuery<'_>,
    budget: &EvalBudget,
    hook: impl FnMut(Window<'_>),
) -> Result<LayeredRanking, LayeredRefusal> {
    budget.check().map_err(|_| OracleRefusal::BudgetExhausted)?;
    if let Some(layer) = request
        .layers
        .iter()
        .find(|layer| layer.precedence.base_epoch != request.generation.generation_epoch)
    {
        return Err(LayeredRefusal::Epoch {
            layers: layer.precedence.base_epoch,
            generation: request.generation.generation_epoch,
        });
    }
    let resolved = resolve::resolve(request.layers, request.max_entries)?;
    let mut account = LayerAccount {
        winners: resolved.winners.len(),
        superseded: resolved.superseded,
        masked: resolved.masked,
        revoked: 0,
        unvisited: 0,
    };
    let mut source = ResolvedRows {
        layers: request.layers,
        winners: resolved.winners,
        next: 0,
        revoked: 0,
        exhausted: false,
    };
    let walk = Walk {
        generation: request.generation,
        layout: RowLayout {
            dimension: request.generation.vector_dimension,
            metric: request.metric,
            unit_norm_tolerance: request.unit_norm_tolerance,
        },
        query: request.query,
        authority: request.authority,
        bounds: request.bounds,
    };
    let ranking = oracle::walk(conn, kernel, &walk, budget, &mut source, hook)?;
    let remaining = source.winners.len() - source.next;
    account.revoked = source.revoked;
    // Only a walk whose last page had nothing after it knows that the winners past its last row are not live; how the walk then ended does not matter.
    if source.exhausted {
        account.revoked += remaining;
    } else {
        account.unvisited = remaining;
    }
    Ok(LayeredRanking {
        ranking,
        layers: account,
    })
}

#[cfg(test)]
mod tests {
    use super::LIVE_SQL;
    use rusqlite::Connection;

    #[test]
    fn the_live_page_query_steps_the_occurrence_identifier_index() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::BASELINE).unwrap();
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", *LIVE_SQL))
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
