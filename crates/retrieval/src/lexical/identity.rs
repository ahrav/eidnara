//! The identity digests a fixed-order, length-delimited manifest of every mechanical component the indexed terms depend on, so a change to any of them changes the digest even when a particular fixture still analyzes to the same terms.
//! The linked SQLite version is a component: the engine's tokenizer defines the effective term, and the projection pins this identity, so an engine change rebuilds the rows.

use sha2::{Digest, Sha256};

use super::{ANALYSIS_CONTRACT_EPOCH, DETAIL, ORIGINAL_COLUMN, PARTS_COLUMN, TOKENIZER};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnalysisIdentity(String);

impl AnalysisIdentity {
    pub fn current() -> Self {
        Self(format!("{:x}", Sha256::digest(Self::preimage())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length prefixes keep `("a", "bc")` and `("ab", "c")` distinct, which plain concatenation would not.
    /// The Unicode version is included because atom and part boundaries come from the toolchain's `char` classification tables; the SQLite version because `unicode61` folds with the engine's own tables.
    pub fn preimage() -> String {
        let (major, minor, update) = char::UNICODE_VERSION;
        let unicode = format!("{major}.{minor}.{update}");
        let mut preimage = String::new();
        for field in [
            "epoch",
            ANALYSIS_CONTRACT_EPOCH,
            "unicode",
            &unicode,
            "tokenizer",
            TOKENIZER,
            "detail",
            DETAIL,
            "columns",
            ORIGINAL_COLUMN,
            PARTS_COLUMN,
            "sqlite",
            rusqlite::version(),
        ] {
            preimage.push_str(&field.len().to_string());
            preimage.push(':');
            preimage.push_str(field);
            preimage.push('\n');
        }
        preimage
    }
}

impl std::fmt::Display for AnalysisIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
