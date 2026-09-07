//! Lossless sidecar metadata for decoding and re-encoding harness messages.
//!
//! Decoding stamps native origins and content fingerprints onto wire blocks. Encoding
//! aligns surviving blocks to native metadata with an order-preserving maximum-score
//! walk, preferring exact origin matches over fingerprint-only matches.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::wire::{IngressMessage, WireBlock};

/// Compaction marker decoded from a harness transcript.
///
/// `ordinal` is the harness message order. Optional part and entry indices identify a
/// boundary nested within that message. `raw` preserves provider fields verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedBoundary {
    pub harness: String,
    pub message_id: String,
    pub ordinal: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    pub raw: Value,
}

/// Decoded messages plus metadata required for lossless re-encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedHarnessMessages {
    pub messages: Vec<IngressMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ExtractedBoundary>,
    pub sidecar: DecodeSidecar,
}

/// Harness metadata indexed by message ID with a separate decode-order list.
///
/// Repeated message IDs replace metadata without changing their first-seen position.
/// `mid_pins` retains stable-key assignments across encode cycles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeSidecar {
    pub harness: String,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub messages: BTreeMap<String, Arc<HarnessMessageMeta>>,
    #[serde(default)]
    pub mid_pins: BTreeMap<String, String>,
}

impl DecodeSidecar {
    pub fn new(harness: impl Into<String>) -> Self {
        Self {
            harness: harness.into(),
            order: Vec::new(),
            messages: BTreeMap::new(),
            mid_pins: BTreeMap::new(),
        }
    }

    /// Inserts or replaces message metadata while preserving first-seen order.
    pub fn remember_message(&mut self, mid: String, meta: HarnessMessageMeta) {
        if !self.messages.contains_key(&mid) {
            self.order.push(mid.clone());
        }
        self.messages.insert(mid, Arc::new(meta));
    }

    pub fn message_by_mid(&self, mid: &str) -> Option<&HarnessMessageMeta> {
        self.messages.get(mid).map(Arc::as_ref)
    }

    /// Returns metadata at zero-based decode order `index`.
    pub fn message_for_index(&self, index: usize) -> Option<&HarnessMessageMeta> {
        self.order
            .get(index)
            .and_then(|mid| self.messages.get(mid.as_str()))
            .map(Arc::as_ref)
    }

    pub fn inherit_pin(&self, stable_key: &str) -> Option<String> {
        self.mid_pins.get(stable_key).cloned()
    }

    pub fn pin_mid(&mut self, stable_key: impl Into<String>, mid: impl Into<String>) {
        self.mid_pins.insert(stable_key.into(), mid.into());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessMessageMeta {
    pub mid: String,
    pub ordinal: u64,
    pub role: String,
    pub raw: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_key: Option<String>,
    #[serde(default)]
    pub blocks: Vec<BlockMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockMeta {
    pub block_index: usize,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_fingerprint: Option<String>,
    pub raw: Value,
}

/// Order-preserving block-to-metadata alignment and native-part retention sets.
pub(crate) struct MatchedBlockMetas<'a> {
    pub(crate) by_block: Vec<Option<&'a BlockMeta>>,
    retained_native_indices: BTreeSet<usize>,
    decoded_native_indices: BTreeSet<usize>,
}

impl MatchedBlockMetas<'_> {
    /// Removes native parts whose decoded blocks were removed.
    ///
    /// Parts with no decoded metadata remain because the sidecar cannot prove they
    /// correspond to a deleted block. Relative order of retained parts is unchanged.
    pub(crate) fn remove_unretained_native_parts<T>(&self, parts: Vec<T>) -> Vec<T> {
        parts
            .into_iter()
            .enumerate()
            .filter_map(|(native_index, part)| {
                let decoded_block_was_removed = self.decoded_native_indices.contains(&native_index)
                    && !self.retained_native_indices.contains(&native_index);
                (!decoded_block_was_removed).then_some(part)
            })
            .collect()
    }
}

const BLOCK_IDENTITY_NAMESPACE: &str = "_eidnara_codec";
const BLOCK_INDEX_KEY: &str = "blockIndex";
const NATIVE_INDEX_KEY: &str = "nativeIndex";
const FINGERPRINT_KEY: &str = "decodedFingerprint";

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct AlignmentScore {
    origin_matches: usize,
    total_matches: usize,
}

impl AlignmentScore {
    fn with_match(self, origin_match: bool) -> Self {
        Self {
            origin_matches: self.origin_matches + usize::from(origin_match),
            total_matches: self.total_matches + 1,
        }
    }
}

/// Hashes decoded content after removing codec-owned identity metadata.
pub(crate) fn decoded_block_fingerprint(block: &WireBlock) -> String {
    let mut canonical = block.clone();
    canonical.provider_extras.remove(BLOCK_IDENTITY_NAMESPACE);
    canonical.mark_modified();
    stable_hash(&serde_json::to_value(canonical).unwrap_or(Value::Null))
}

/// Stores a block's decoded origin and pre-mutation fingerprint in provider extras.
pub(crate) fn stamp_block_identity(
    block: &mut WireBlock,
    block_index: usize,
    native_index: usize,
    fingerprint: &str,
) {
    let identity = block
        .provider_extras
        .entry(BLOCK_IDENTITY_NAMESPACE.to_string())
        .or_default();
    identity.insert(BLOCK_INDEX_KEY.to_string(), Value::from(block_index));
    identity.insert(NATIVE_INDEX_KEY.to_string(), Value::from(native_index));
    identity.insert(
        FINGERPRINT_KEY.to_string(),
        Value::String(fingerprint.to_string()),
    );
    block.mark_modified();
}

fn stamped_block_identity(block: &WireBlock) -> Option<(usize, usize, &str)> {
    let identity = block.provider_extras.get(BLOCK_IDENTITY_NAMESPACE)?;
    let block_index = identity.get(BLOCK_INDEX_KEY)?.as_u64()?.try_into().ok()?;
    let native_index = identity.get(NATIVE_INDEX_KEY)?.as_u64()?.try_into().ok()?;
    let fingerprint = identity.get(FINGERPRINT_KEY)?.as_str()?;
    Some((block_index, native_index, fingerprint))
}

/// True when a decoded block retains its exact native-part origin.
pub(crate) fn has_stamped_block_identity(block: &WireBlock) -> bool {
    stamped_block_identity(block).is_some()
}

pub(crate) fn block_is_unchanged(block: &WireBlock, meta: &BlockMeta) -> bool {
    meta.content_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| decoded_block_fingerprint(block) == fingerprint)
}

fn alignment_candidate(
    block: &WireBlock,
    block_index: usize,
    meta: &BlockMeta,
    kind_matches: bool,
) -> Option<bool> {
    if let Some((origin_block_index, origin_native_index, fingerprint)) =
        stamped_block_identity(block)
    {
        let origin_matches = origin_block_index == meta.block_index
            && Some(origin_native_index) == meta.native_index
            && meta.content_fingerprint.as_deref() == Some(fingerprint);
        return origin_matches.then_some(true);
    }

    if kind_matches
        && meta
            .content_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| decoded_block_fingerprint(block) == fingerprint)
    {
        return Some(false);
    }

    // Position-only matching for fingerprintless sidecars requires an unchanged block index and does not scan nearby same-kind blocks.
    (meta.content_fingerprint.is_none() && block_index == meta.block_index && kind_matches)
        .then_some(false)
}

/// Largest `blocks * metas` product the optimal alignment may allocate matrices for.
///
/// The score matrix costs two words per cell, so this caps it near 16 MiB; a single message
/// with thousands of blocks on both sides exceeds it and takes the linear-memory greedy path.
const MAX_ALIGNMENT_CELLS: usize = 1 << 20;

/// Aligns blocks with metadata without reordering either sequence.
///
/// Exact stamped origins score above fingerprint-only or positional matches. Dynamic
/// programming maximizes origin matches first and total matches second. Each block and
/// metadata row appears in at most one pair. Time and memory are `O(blocks * metas)` up to
/// [`MAX_ALIGNMENT_CELLS`]; above that, a forward greedy walk pairs each block with the
/// first unpaired candidate at `O(blocks + metas)` memory, trading optimality for a bound.
pub(crate) fn match_block_metas<'a>(
    blocks: &[WireBlock],
    metas: &'a [BlockMeta],
    mut matches: impl FnMut(&WireBlock, &BlockMeta) -> bool,
) -> MatchedBlockMetas<'a> {
    let cells = blocks.len().saturating_mul(metas.len());
    let by_block = if cells > MAX_ALIGNMENT_CELLS {
        greedy_block_metas(blocks, metas, &mut matches)
    } else {
        optimal_block_metas(blocks, metas, &mut matches)
    };

    let retained_native_indices = by_block
        .iter()
        .filter_map(|meta| meta.and_then(|meta| meta.native_index))
        .collect();
    let decoded_native_indices = metas.iter().filter_map(|meta| meta.native_index).collect();

    MatchedBlockMetas {
        by_block,
        retained_native_indices,
        decoded_native_indices,
    }
}

fn greedy_block_metas<'a>(
    blocks: &[WireBlock],
    metas: &'a [BlockMeta],
    matches: &mut impl FnMut(&WireBlock, &BlockMeta) -> bool,
) -> Vec<Option<&'a BlockMeta>> {
    // Every candidate pairing is reachable through one of three keys, so indexing the metas
    // once keeps each block to a handful of lookups instead of a rescan of the remaining
    // suffix, and each block is fingerprinted at most once.
    let mut by_fingerprint: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut by_block_index: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (meta_index, meta) in metas.iter().enumerate() {
        match meta.content_fingerprint.as_deref() {
            Some(fingerprint) => by_fingerprint
                .entry(fingerprint)
                .or_default()
                .push(meta_index),
            None => by_block_index
                .entry(meta.block_index)
                .or_default()
                .push(meta_index),
        }
    }
    // Meta indices are pushed in order, so each list is sorted and the suffix at or after
    // the cursor is a contiguous tail.
    let first_at_or_after =
        |indices: Option<&Vec<usize>>, cursor: usize, accept: &mut dyn FnMut(usize) -> bool| {
            indices.and_then(|indices| {
                let position = indices.partition_point(|index| *index < cursor);
                indices[position..]
                    .iter()
                    .copied()
                    .find(|index| accept(*index))
            })
        };

    let mut by_block = vec![None; blocks.len()];
    let mut meta_cursor = 0;
    for (block_index, block) in blocks.iter().enumerate() {
        let paired = if let Some((_, _, fingerprint)) = stamped_block_identity(block) {
            first_at_or_after(
                by_fingerprint.get(fingerprint),
                meta_cursor,
                &mut |meta_index| {
                    let meta = &metas[meta_index];
                    alignment_candidate(block, block_index, meta, matches(block, meta)).is_some()
                },
            )
        } else {
            let fingerprint = decoded_block_fingerprint(block);
            let mut kind_matches = |meta_index: usize| matches(block, &metas[meta_index]);
            first_at_or_after(
                by_fingerprint.get(fingerprint.as_str()),
                meta_cursor,
                &mut kind_matches,
            )
            .or_else(|| {
                first_at_or_after(
                    by_block_index.get(&block_index),
                    meta_cursor,
                    &mut kind_matches,
                )
            })
        };
        if let Some(meta_index) = paired {
            by_block[block_index] = Some(&metas[meta_index]);
            meta_cursor = meta_index + 1;
        }
    }
    by_block
}

fn optimal_block_metas<'a>(
    blocks: &[WireBlock],
    metas: &'a [BlockMeta],
    matches: &mut impl FnMut(&WireBlock, &BlockMeta) -> bool,
) -> Vec<Option<&'a BlockMeta>> {
    let mut candidates = vec![vec![None; metas.len()]; blocks.len()];
    for (block_index, block) in blocks.iter().enumerate() {
        for (meta_index, meta) in metas.iter().enumerate() {
            let kind_matches = matches(block, meta);
            candidates[block_index][meta_index] =
                alignment_candidate(block, block_index, meta, kind_matches);
        }
    }

    // Origin indexes are stamped onto decoded blocks and survive reductions, overlays, and deletion compaction in WireBlock::provider_extras.
    // Each origin index stores the pre-mutation decoded fingerprint.
    // Pre-mutation fingerprints align mutated survivors with their native metadata; the LCS-style walk preserves native order and avoids same-kind adjacency matching.
    let mut scores = vec![vec![AlignmentScore::default(); metas.len() + 1]; blocks.len() + 1];
    for block_index in (0..blocks.len()).rev() {
        for meta_index in (0..metas.len()).rev() {
            let mut best =
                scores[block_index + 1][meta_index].max(scores[block_index][meta_index + 1]);
            if let Some(origin_match) = candidates[block_index][meta_index] {
                best = best.max(scores[block_index + 1][meta_index + 1].with_match(origin_match));
            }
            scores[block_index][meta_index] = best;
        }
    }

    let mut by_block = vec![None; blocks.len()];
    let (mut block_index, mut meta_index) = (0, 0);
    while block_index < blocks.len() && meta_index < metas.len() {
        if let Some(origin_match) = candidates[block_index][meta_index] {
            let matched_score = scores[block_index + 1][meta_index + 1].with_match(origin_match);
            if matched_score == scores[block_index][meta_index] {
                by_block[block_index] = Some(&metas[meta_index]);
                block_index += 1;
                meta_index += 1;
                continue;
            }
        }
        if scores[block_index + 1][meta_index] >= scores[block_index][meta_index + 1] {
            block_index += 1;
        } else {
            meta_index += 1;
        }
    }

    by_block
}

/// Returns the full lowercase SHA-256 digest of `serde_json`-serialized bytes.
///
/// Serialization failure hashes an empty byte sequence.
pub fn stable_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    hex_prefix(&digest, digest.len())
}

/// Returns up to `chars` lowercase hexadecimal characters from serialized JSON's SHA-256 digest.
///
/// Serialization failure hashes an empty byte sequence. Requests above 64 characters
/// return the full 64-character digest.
pub fn stable_hash_prefix(value: &Value, chars: usize) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    hex_prefix(&digest, chars.div_ceil(2))
        .chars()
        .take(chars)
        .collect()
}

fn hex_prefix(bytes: &[u8], count: usize) -> String {
    let mut out = String::with_capacity(count * 2);
    for byte in bytes.iter().take(count) {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Selects metadata by explicit harness ID, then by decode order for nonsynthetic messages.
///
/// Synthetic messages never use the positional fallback.
pub fn meta_for_ck<'a>(
    sidecar: &'a DecodeSidecar,
    msg: &'a crate::wire::WireMessage,
    index: usize,
) -> Option<&'a HarnessMessageMeta> {
    msg.meta
        .harness_id
        .as_deref()
        .and_then(|mid| sidecar.message_by_mid(mid))
        .or_else(|| {
            (!msg.meta.synthetic)
                .then(|| sidecar.message_for_index(index))
                .flatten()
        })
}

/// Recognizes either supported boolean marker for a synthetic native part.
pub(crate) fn is_synthetic_part(part: &Value) -> bool {
    part.get("synthetic")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || part
            .get("syntheticTodoMarker")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::BlockKind;

    fn text_block(text: &str) -> WireBlock {
        WireBlock::bare(BlockKind::Text {
            text: text.to_string(),
        })
    }

    fn text_meta(block_index: usize) -> BlockMeta {
        BlockMeta {
            block_index,
            kind: "text".to_string(),
            native_index: Some(block_index),
            native_id: None,
            item_id: None,
            content_fingerprint: None,
            raw: Value::Null,
        }
    }

    #[test]
    fn oversized_alignment_takes_the_linear_memory_path_and_keeps_positional_pairs() {
        // 1,100 x 1,100 exceeds MAX_ALIGNMENT_CELLS; the optimal path would allocate over
        // 1.2M score cells. The greedy walk must still pair every positional match in order.
        let count = 1_100;
        assert!(count * count > MAX_ALIGNMENT_CELLS);
        let blocks = (0..count)
            .map(|index| text_block(&format!("block {index}")))
            .collect::<Vec<_>>();
        let metas = (0..count).map(text_meta).collect::<Vec<_>>();
        let matched = match_block_metas(&blocks, &metas, |_, meta| meta.kind == "text");
        for (index, meta) in matched.by_block.iter().enumerate() {
            assert_eq!(
                meta.map(|meta| meta.block_index),
                Some(index),
                "block {index} must pair with its positional meta"
            );
        }
        assert_eq!(matched.retained_native_indices.len(), count);
    }

    #[test]
    fn greedy_alignment_never_reuses_a_meta_and_preserves_order() {
        let blocks = vec![text_block("a"), text_block("b"), text_block("c")];
        // Only block 1 has a positional meta; blocks 0 and 2 have none.
        let metas = vec![text_meta(1)];
        let by_block = greedy_block_metas(&blocks, &metas, &mut |_, meta| meta.kind == "text");
        assert_eq!(by_block[0].map(|meta| meta.block_index), None);
        assert_eq!(by_block[1].map(|meta| meta.block_index), Some(1));
        assert_eq!(by_block[2].map(|meta| meta.block_index), None);
    }

    #[test]
    fn greedy_alignment_pairs_fingerprinted_metas_through_the_fingerprint_index() {
        // Two fingerprinted metas describe blocks that now sit after the positional ones; an
        // index-only walk would never reach them, and the fingerprint index must, in order and
        // without reusing a meta for the duplicate trailing copy.
        let moved = [text_block("moved a"), text_block("moved b")];
        let mut metas = (0..2).map(text_meta).collect::<Vec<_>>();
        for block in &moved {
            metas.push(BlockMeta {
                block_index: 0,
                content_fingerprint: Some(decoded_block_fingerprint(block)),
                ..text_meta(0)
            });
        }
        let mut blocks = vec![text_block("block 0"), text_block("block 1")];
        blocks.extend(moved.iter().cloned());
        blocks.push(text_block("moved a"));

        let by_block = greedy_block_metas(&blocks, &metas, &mut |_, meta| meta.kind == "text");
        assert_eq!(by_block[0].map(|meta| meta.block_index), Some(0));
        assert_eq!(by_block[1].map(|meta| meta.block_index), Some(1));
        assert_eq!(
            by_block[2].and_then(|meta| meta.content_fingerprint.as_deref()),
            Some(decoded_block_fingerprint(&moved[0]).as_str())
        );
        assert_eq!(
            by_block[3].and_then(|meta| meta.content_fingerprint.as_deref()),
            Some(decoded_block_fingerprint(&moved[1]).as_str())
        );
        assert!(by_block[4].is_none(), "a meta pairs with at most one block");

        // Fingerprinted metas ahead of the positional ones fall behind the cursor once the
        // positional blocks pair, so the moved blocks find nothing: the walk never revisits.
        metas.rotate_left(2);
        let by_block = greedy_block_metas(&blocks, &metas, &mut |_, meta| meta.kind == "text");
        assert_eq!(by_block[0].map(|meta| meta.block_index), Some(0));
        assert_eq!(by_block[1].map(|meta| meta.block_index), Some(1));
        assert!(by_block[2..].iter().all(Option::is_none));
    }
}
