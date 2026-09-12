//! A reusable cache prefix ends at the first divergent served block.
//!
//! Appending blocks preserves the reusable prefix and is not divergence.
//! An empty prior sequence represents a cold start.

use memory_store::ServedBlockFingerprint;
use serde::{Deserialize, Serialize};

/// `DivergenceKind` classifies why a served prefix is no longer reusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    /// Block identity is stable, but its content fingerprint changed.
    ContentChanged,
    /// New sequence introduced a block before the expected old block.
    Inserted,
    /// New sequence omitted the expected old block.
    Removed,
    /// Both sequences continue with incompatible block order or identity.
    Reordered,
}

/// `FirstDivergence` identifies the boundary before which the old prefix remains reusable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstDivergence {
    /// Zero-based position of the first incompatible block.
    pub index: usize,
    /// Old block identifier, or `None` when no old block occupies the position.
    pub block_id_old: Option<String>,
    /// New block identifier, or `None` when the new sequence ended.
    pub block_id_new: Option<String>,
    /// Classification derived from block identity and later occurrences.
    pub kind: DivergenceKind,
    /// Approximate tokens before `index`, using four serialized bytes per token.
    pub approx_token_depth: usize,
}

/// `first_divergence` returns the first boundary that prevents reusing `old`'s prefix.
///
/// An empty old sequence is a cold start. A new sequence that preserves every old block in order
/// and only appends blocks returns `None`.
pub fn first_divergence(
    old: &[ServedBlockFingerprint],
    new: &[ServedBlockFingerprint],
) -> Option<FirstDivergence> {
    if old.is_empty() {
        return None;
    }

    for index in 0..old.len().min(new.len()) {
        let old_block = &old[index];
        let new_block = &new[index];
        if old_block == new_block {
            continue;
        }

        let kind = if old_block.block_id == new_block.block_id {
            DivergenceKind::ContentChanged
        } else {
            let old_id_reappears = new[index..]
                .iter()
                .any(|block| block.block_id == old_block.block_id);
            let new_id_reappears = old[index..]
                .iter()
                .any(|block| block.block_id == new_block.block_id);
            match (old_id_reappears, new_id_reappears) {
                (true, false) => DivergenceKind::Inserted,
                (false, true) => DivergenceKind::Removed,
                (true, true) | (false, false) => DivergenceKind::Reordered,
            }
        };

        return Some(FirstDivergence {
            index,
            block_id_old: Some(old_block.block_id.clone()),
            block_id_new: Some(new_block.block_id.clone()),
            kind,
            approx_token_depth: approx_token_depth(old, new, index),
        });
    }

    if new.len() < old.len() {
        let index = new.len();
        let old_block = &old[index];
        return Some(FirstDivergence {
            index,
            block_id_old: Some(old_block.block_id.clone()),
            block_id_new: None,
            kind: DivergenceKind::Removed,
            approx_token_depth: approx_token_depth(old, new, index),
        });
    }

    None
}

fn approx_token_depth(
    old: &[ServedBlockFingerprint],
    new: &[ServedBlockFingerprint],
    index: usize,
) -> usize {
    let old_bytes = old.iter().take(index).fold(0usize, |total, block| {
        total.saturating_add(block.serialized_len)
    });
    let new_bytes = new.iter().take(index).fold(0usize, |total, block| {
        total.saturating_add(block.serialized_len)
    });
    old_bytes.max(new_bytes).saturating_add(3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, hash: &str, len: usize) -> ServedBlockFingerprint {
        ServedBlockFingerprint {
            block_id: id.to_string(),
            content_hash: hash.to_string(),
            serialized_len: len,
        }
    }

    #[test]
    fn first_divergence_classifies_each_boundary_kind_and_ignores_appends() {
        let a = block("a", "a", 4);
        let b = block("b", "b", 4);
        let c = block("c", "c", 4);
        let x = block("x", "x", 4);
        // `None` means the old sequence is a reusable prefix of the new sequence.
        type Expected<'a> = Option<(usize, DivergenceKind, &'a str, Option<&'a str>)>;
        let cases: [(Vec<_>, Vec<_>, Expected<'_>); 8] = [
            (
                vec![block("a", "one", 8), block("b", "two", 8)],
                vec![block("a", "changed", 12), block("b", "two", 8)],
                Some((0, DivergenceKind::ContentChanged, "a", Some("a"))),
            ),
            (
                vec![a.clone(), b.clone(), c.clone()],
                vec![a.clone(), x.clone(), b.clone(), c.clone()],
                Some((1, DivergenceKind::Inserted, "b", Some("x"))),
            ),
            (
                vec![a.clone(), b.clone(), c.clone()],
                vec![a.clone(), c.clone()],
                Some((1, DivergenceKind::Removed, "b", Some("c"))),
            ),
            // The new sequence ends before the old one: the missing block has no new id.
            (
                vec![a.clone(), b.clone(), c.clone()],
                vec![a.clone(), b.clone()],
                Some((2, DivergenceKind::Removed, "c", None)),
            ),
            (
                vec![a.clone(), b.clone(), c.clone()],
                vec![b.clone(), a.clone(), c.clone()],
                Some((0, DivergenceKind::Reordered, "a", Some("b"))),
            ),
            (
                vec![a.clone(), b.clone()],
                vec![a.clone(), b.clone(), c.clone()],
                None,
            ),
            (vec![a.clone(), b.clone()], vec![a.clone(), b.clone()], None),
            // An empty old sequence is a cold start.
            (vec![], vec![a.clone()], None),
        ];
        for (old, new, expected) in cases {
            let divergence = first_divergence(&old, &new);
            let observed = divergence.as_ref().map(|divergence| {
                (
                    divergence.index,
                    divergence.kind,
                    divergence
                        .block_id_old
                        .as_deref()
                        .expect("a divergence names its old block"),
                    divergence.block_id_new.as_deref(),
                )
            });
            assert_eq!(observed, expected, "old={old:?} new={new:?}");
        }
    }
}
