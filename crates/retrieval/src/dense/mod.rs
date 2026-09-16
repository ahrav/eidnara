//! Original-f32 rows and the exhaustive ranking over them.
//!
//! The codec fixes how a row is stored and which rows a generation admits; the scorer fixes the arithmetic and the order; the oracle walks every live row that requires a vector, judges canonical eligibility in bounded batches, and reports coverage shortfalls and authority changes instead of a complete result.
//! The scalar recipe calibrates, encodes, and scores int8 codes whose ranking is checked against that oracle.
//! The resolver turns one base layer and its ordered deltas into the current occurrence set, and the layered ranking walks the oracle's path over those winners.

pub mod codec;
pub mod export;
pub mod layered;
pub mod oracle;
pub mod resolve;
pub mod scalar;
pub mod score;

pub use codec::{Metric, RowLayout, RowRejection};
pub use layered::{LayerAccount, LayeredQuery, LayeredRanking, LayeredRefusal, rank_layers};
pub use oracle::{
    Completion, Consumed, DenseCoverage, ExhaustiveQuery, ExhaustiveRanking, IncompleteReason,
    OracleBounds, OracleRefusal, Window, exhaustive,
};
pub use resolve::{Layer, Precedence, ResolveRefusal, Resolved, Winner, resolve};
pub use score::{Ranked, inner_product, rank_order, rescore, score};
