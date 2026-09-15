//! Original-f32 rows and the exhaustive ranking over them.
//!
//! The codec fixes how a row is stored and which rows a generation admits; the scorer fixes the arithmetic and the order; the oracle walks every live row that requires a vector, judges canonical eligibility in bounded batches, and reports coverage shortfalls and authority changes instead of a complete result.
//! The scalar recipe calibrates, encodes, and scores int8 codes whose ranking is checked against that oracle.

pub mod codec;
pub mod export;
pub mod oracle;
pub mod scalar;
pub mod score;

pub use codec::{Metric, RowLayout, RowRejection};
pub use oracle::{
    Completion, Consumed, DenseCoverage, ExhaustiveQuery, ExhaustiveRanking, IncompleteReason,
    OracleBounds, OracleRefusal, Window, exhaustive,
};
pub use score::{Ranked, inner_product, rank_order, rescore, score};
