//! Indexing and querying share one identifier analyzer and one literal MATCH compiler.
//! The engine tokenizes analyzer output on both sides, so a probe and the indexed text of the same identifier always reach the engine in the same form.
//!
//! [`AnalysisIdentity`] changes whenever the contract epoch, the `char::UNICODE_VERSION` of the toolchain, the tokenizer, the detail mode, or the indexed columns change.
//! Changes to analysis or compilation rules not represented by another identity component require a new [`ANALYSIS_CONTRACT_EPOCH`].
//!
//! Folded FTS terms are recall candidates only; they are never byte identity or authorization evidence.
//! Canonical validation decides eligibility.

pub mod analysis;
pub mod compile;
pub mod identity;
pub mod index;

pub use analysis::{Analysis, LexicalBounds, LexicalRefusal, analyze, analyze_segments};
pub use compile::{Probe, compile};
pub use identity::AnalysisIdentity;
pub use index::{
    EngineIdentity, OCCURRENCE_ID_COLUMN, ROWID_WORDS, probe_engine, rowid, rowids, verify_rows,
};

/// `_` is a token character so `snake_case` stays one engine token, and `remove_diacritics 2` folds precomposed Latin diacritics.
pub const TOKENIZER: &str = "unicode61 remove_diacritics 2 tokenchars '_'";

/// A quoted atom the engine splits into several tokens is a phrase query, and `fts5vocab` reports offsets, both of which need full detail.
pub const DETAIL: &str = "full";

pub const ORIGINAL_COLUMN: &str = "original";

pub const PARTS_COLUMN: &str = "parts";

/// Changes whenever an analysis or compilation rule changes without changing any other identity component.
pub const ANALYSIS_CONTRACT_EPOCH: &str = "identifier-analysis.v1";

/// The index and every scratch oracle table use these `fts5(...)` arguments so their tokenizer and detail mode cannot drift.
pub fn fts5_table_args() -> String {
    format!(
        "{ORIGINAL_COLUMN}, {PARTS_COLUMN}, {OCCURRENCE_ID_COLUMN} UNINDEXED, tokenize = '{}', detail = {DETAIL}",
        TOKENIZER.replace('\'', "''")
    )
}
