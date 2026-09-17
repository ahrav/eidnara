//! Products are formed in f64 and summed in increasing coordinate order; Rust never contracts `a * b + c` into a fused multiply-add, so every scoring path yields the same f64 for the same rows.
//! Ranking compares score descending, then occurrence identifier bytes ascending, with no epsilon.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::num::NonZeroUsize;

use kernel::source_identity::OccurrenceClass;

use super::codec::{self, Metric, RowLayout, RowRejection};

/// Panics on unequal lengths so a shape error can never become a silently truncated score.
pub fn inner_product(query: &[f32], row: &[f32]) -> f64 {
    assert_eq!(query.len(), row.len(), "rows of one layout have one length");
    let mut sum = 0.0f64;
    for (q, r) in query.iter().zip(row) {
        let product = f64::from(*q) * f64::from(*r);
        sum += product;
    }
    sum
}

/// The metric is matched exhaustively so a new variant cannot fall through to the wrong arithmetic.
pub fn score(metric: Metric, query: &[f32], row: &[f32]) -> f64 {
    match metric {
        Metric::InnerProduct => inner_product(query, row),
    }
}

/// Eight rows give eight independent accumulation chains while each row stays accumulated in coordinate order.
pub const BLOCK_ROWS: usize = 8;

/// One entry per lane; a struct of arrays because an array of per-row structs makes the compiler interleave the two accumulators and lose the lane-parallel code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockSums {
    /// Accumulation starts at `+0.0` and visits squared coordinates in increasing order.
    pub sums_of_squares: [f64; BLOCK_ROWS],
    pub scores: [f64; BLOCK_ROWS],
}

/// Each lane accumulates coordinates in order, so finite-row scores match [`inner_product`] bit for bit.
///
/// Panics on unequal lengths so a shape error can never become a silently truncated score.
pub fn inner_product_block(query: &[f32], rows: &[&[f32]; BLOCK_ROWS]) -> BlockSums {
    for row in rows {
        assert_eq!(query.len(), row.len(), "rows of one layout have one length");
    }
    let mut squares = [0.0f64; BLOCK_ROWS];
    let mut scores = [0.0f64; BLOCK_ROWS];
    let tiled = query.len() / 4 * 4;
    let mut start = 0;
    while start < tiled {
        // Four coordinates of each row per tile give the compiler contiguous loads to interleave across lanes.
        let tile: [[f32; 4]; BLOCK_ROWS] = std::array::from_fn(|lane| {
            let row = rows[lane];
            [row[start], row[start + 1], row[start + 2], row[start + 3]]
        });
        let q: [f64; 4] = std::array::from_fn(|offset| f64::from(query[start + offset]));
        for offset in 0..4 {
            for lane in 0..BLOCK_ROWS {
                let widened = f64::from(tile[lane][offset]);
                squares[lane] += widened * widened;
                let product = q[offset] * widened;
                scores[lane] += product;
            }
        }
        start += 4;
    }
    for (coordinate, q) in query.iter().enumerate().skip(tiled) {
        let q = f64::from(*q);
        for lane in 0..BLOCK_ROWS {
            let widened = f64::from(rows[lane][coordinate]);
            squares[lane] += widened * widened;
            let product = q * widened;
            scores[lane] += product;
        }
    }
    BlockSums {
        sums_of_squares: squares,
        scores,
    }
}

/// [`score`] for a block of rows; the metric is matched exhaustively like the single-row form.
pub fn score_block(metric: Metric, query: &[f32], rows: &[&[f32]; BLOCK_ROWS]) -> BlockSums {
    match metric {
        Metric::InnerProduct => inner_product_block(query, rows),
    }
}

/// `total_cmp` orders finite scores; no NaN reaches it because rows are validated finite.
pub fn rank_order(left: (f64, &str), right: (f64, &str)) -> Ordering {
    right
        .0
        .total_cmp(&left.0)
        .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub occurrence_id: String,
    pub class: OccurrenceClass,
    pub score: f64,
}

impl Ranked {
    fn key(&self) -> (f64, &str) {
        (self.score, &self.occurrence_id)
    }
}

/// Orders on the ranking alone so the heap's maximum is the worst-ranked member; the payload rides along unordered.
struct Worst<T>(Ranked, T);

impl<T> PartialEq for Worst<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T> Eq for Worst<T> {}

impl<T> PartialOrd for Worst<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Worst<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        rank_order(self.0.key(), other.0.key())
    }
}

pub struct TopK<T> {
    k: NonZeroUsize,
    heap: BinaryHeap<Worst<T>>,
}

impl<T> TopK<T> {
    pub fn new(k: NonZeroUsize) -> Self {
        Self {
            k,
            heap: BinaryHeap::with_capacity(k.get()),
        }
    }

    pub fn admits(&self, score: f64, occurrence_id: &str) -> bool {
        self.heap.len() < self.k.get()
            || self.heap.peek().is_some_and(|worst| {
                rank_order((score, occurrence_id), worst.0.key()) == Ordering::Less
            })
    }

    pub fn k(&self) -> usize {
        self.k.get()
    }

    /// A row that ranks at or below the current worst member when the set is full is dropped.
    pub fn offer(&mut self, ranked: Ranked, payload: T) {
        if self.heap.len() < self.k.get() {
            self.heap.push(Worst(ranked, payload));
            return;
        }
        if self.admits(ranked.score, &ranked.occurrence_id) {
            self.heap.pop();
            self.heap.push(Worst(ranked, payload));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Best first.
    pub fn into_ranked(self) -> Vec<(Ranked, T)> {
        self.heap
            .into_sorted_vec()
            .into_iter()
            .map(|worst| (worst.0, worst.1))
            .collect()
    }
}

/// Validates the query and every row against `layout`, then returns all rows best first.
///
/// # Errors
///
/// The first row, or the query, that fails `layout` is returned and nothing is scored.
pub fn rescore<'a>(
    layout: &RowLayout,
    query: &[f32],
    rows: impl IntoIterator<Item = (&'a str, OccurrenceClass, &'a [f32])>,
) -> Result<Vec<Ranked>, RowRejection> {
    layout.check()?;
    codec::validate(query, layout)?;
    let mut ranked = Vec::new();
    for (occurrence_id, class, row) in rows {
        codec::validate(row, layout)?;
        ranked.push(Ranked {
            occurrence_id: occurrence_id.to_owned(),
            class,
            score: score(layout.metric, query, row),
        });
    }
    ranked.sort_by(|left, right| rank_order(left.key(), right.key()));
    Ok(ranked)
}
