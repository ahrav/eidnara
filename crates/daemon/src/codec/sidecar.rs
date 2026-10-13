//! Lossless sidecar metadata for decoding and re-encoding harness messages.
//!
//! Decoding records each native message's envelope, block metadata, and decoded blocks.
//! Encoding aligns surviving blocks to that metadata by canonical content with an
//! order-preserving maximum-match walk, or a forward greedy walk above
//! `MAX_ALIGNMENT_CELLS`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::wire::{
    BlockKind, IngressMessage, MediaBlock, OpaqueBlock, OutputKind, ProviderExtras,
    ResultBlockKind, WireBlock,
};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeSidecar {
    pub harness: String,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub messages: BTreeMap<String, Arc<HarnessMessageMeta>>,
}

impl DecodeSidecar {
    pub fn new(harness: impl Into<String>) -> Self {
        Self {
            harness: harness.into(),
            order: Vec::new(),
            messages: BTreeMap::new(),
        }
    }

    /// Inserts or replaces message metadata while preserving first-seen order.
    pub fn remember_message(&mut self, mid: String, meta: HarnessMessageMeta) {
        match self.messages.entry(mid) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                self.order.push(entry.key().clone());
                entry.insert(Arc::new(meta));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.insert(Arc::new(meta));
            }
        }
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessMessageMeta {
    pub mid: String,
    pub ordinal: u64,
    pub role: String,
    pub raw: Arc<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_key: Option<String>,
    #[serde(default)]
    pub blocks: Vec<BlockMeta>,
    /// The message's decoded blocks in decode order, which each block meta's `block_index`
    /// indexes; alignment compares served blocks against them. The OpenCode sidecar fills it.
    /// The Pi codec keeps it empty and hands each encode the blocks it decodes from the row.
    #[serde(default)]
    pub decoded: Vec<WireBlock>,
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
}

/// `MatchedBlockMetas` pairs blocks with metadata in order and tracks native parts eligible
/// for removal.
pub(crate) struct MatchedBlockMetas<'a> {
    pub(crate) by_block: Vec<Option<&'a BlockMeta>>,
    unretained_native_parts: Vec<bool>,
    decoded: &'a [WireBlock],
}

impl MatchedBlockMetas<'_> {
    /// Whether `blocks[block_index]` is the decoded block `meta` names, compared by canonical
    /// content; `blocks` must be the slice used to build this alignment.
    pub(crate) fn block_is_unchanged(
        &self,
        blocks: &[WireBlock],
        block_index: usize,
        meta: &BlockMeta,
    ) -> bool {
        decoded_matches(self.decoded, &blocks[block_index], meta)
    }

    /// Removes native parts whose decoded blocks were removed.
    ///
    /// Parts with no decoded metadata remain because the sidecar cannot prove they
    /// correspond to a deleted block. Relative order of retained parts is unchanged.
    pub(crate) fn remove_unretained_native_parts<T>(&self, parts: Vec<T>) -> Vec<T> {
        parts
            .into_iter()
            .enumerate()
            .filter_map(|(native_index, part)| {
                let decoded_block_was_removed = self
                    .unretained_native_parts
                    .get(native_index)
                    .copied()
                    .unwrap_or(false);
                (!decoded_block_was_removed).then_some(part)
            })
            .collect()
    }
}

impl<'a> MatchedBlockMetas<'a> {
    /// Positional pairing treats equal-length unmatched runs as in-place edits and restores
    /// their native slots.
    pub(crate) fn pair_in_place_edits(
        &mut self,
        blocks: &[WireBlock],
        metas: &'a [BlockMeta],
        mut matches: impl FnMut(&WireBlock, &BlockMeta) -> bool,
    ) {
        let (mut run_block, mut run_meta) = (0, 0);
        for block_index in 0..=blocks.len() {
            let anchor_meta = match self.by_block.get(block_index) {
                Some(None) => continue,
                Some(Some(anchor)) => {
                    let Some(offset) = metas[run_meta..]
                        .iter()
                        .position(|meta| std::ptr::eq(meta, *anchor))
                    else {
                        return;
                    };
                    run_meta + offset
                }
                None => metas.len(),
            };
            let run_blocks = run_block..block_index;
            let run_metas = run_meta..anchor_meta;
            let in_place = !run_blocks.is_empty()
                && run_blocks.len() == run_metas.len()
                && run_blocks
                    .clone()
                    .zip(run_metas.clone())
                    .all(|(block, meta)| matches(&blocks[block], &metas[meta]));
            if in_place {
                for (block, meta) in run_blocks.zip(run_metas) {
                    self.by_block[block] = Some(&metas[meta]);
                    if let Some(native_index) = metas[meta].native_index {
                        self.unretained_native_parts[native_index] = false;
                    }
                }
            }
            run_block = block_index + 1;
            run_meta = anchor_meta + 1;
        }
    }
}

/// A provider-extras namespace no decoded block carries, excluded from content comparison so
/// a block carrying it compares by its content alone.
pub(crate) const BLOCK_IDENTITY_NAMESPACE: &str = "_eidnara_codec";

/// The block's canonical bytes with the codec namespace removed, the text both alignment
/// regimes compare; a block that cannot serialize reads as the JSON text `null`.
fn canonical_text(block: &WireBlock) -> String {
    let canonical = if block.provider_extras.contains_key(BLOCK_IDENTITY_NAMESPACE) {
        let mut canonical = block.clone();
        canonical.provider_extras.remove(BLOCK_IDENTITY_NAMESPACE);
        Cow::Owned(canonical)
    } else {
        Cow::Borrowed(block)
    };
    crate::served_json::canonical_block_bytes(&canonical).unwrap_or_else(|_| "null".to_string())
}

/// The hex SHA-256 of a block's canonical text, the fingerprint the alignment's content
/// equality is pinned against in tests.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn decoded_block_fingerprint(block: &WireBlock) -> String {
    #[cfg(test)]
    count_digest();
    let digest = Sha256::digest(canonical_text(block).as_bytes());
    hex_prefix(&digest, digest.len())
}

#[cfg(feature = "test-support")]
pub fn decoded_block_fingerprint_for_test(block: &WireBlock) -> String {
    decoded_block_fingerprint(block)
}

#[cfg(feature = "test-support")]
pub const BLOCK_IDENTITY_NAMESPACE_FOR_TEST: &str = BLOCK_IDENTITY_NAMESPACE;

pub(crate) fn equals_decoded_block(block: &WireBlock, decoded: &WireBlock) -> bool {
    fn content(block: &WireBlock) -> impl Iterator<Item = (&String, &BTreeMap<String, Value>)> {
        block
            .provider_extras
            .iter()
            .filter(|(namespace, _)| namespace.as_str() != BLOCK_IDENTITY_NAMESPACE)
    }
    block.kind() == decoded.kind() && content(block).eq(content(decoded))
}

/// Whether `block` is the decoded block `meta` names in `decoded`, compared by canonical
/// content; a meta naming no decoded block matches nothing.
pub(crate) fn decoded_matches(decoded: &[WireBlock], block: &WireBlock, meta: &BlockMeta) -> bool {
    decoded
        .get(meta.block_index)
        .is_some_and(|decoded| same_decoded_content(block, decoded))
}

/// Whether two blocks have equal canonical text, decided without serializing them where the
/// structure decides it.
///
/// Canonical block serialization is injective, and structurally equal blocks serialize to the
/// same bytes unless a float zero is `0.0` on one side and `-0.0` on the other: those compare
/// equal and serialize apart. Equal blocks that hold a float zero therefore compare by their
/// canonical text, so signed zeros stay distinct.
pub(crate) fn same_decoded_content(block: &WireBlock, decoded: &WireBlock) -> bool {
    equals_decoded_block(block, decoded)
        && (!holds_float_zero(block) || canonical_text(block) == canonical_text(decoded))
}

/// Whether `json` or any value nested in it is a float zero of either sign.
fn json_holds_float_zero(json: &Value) -> bool {
    match json {
        Value::Number(number) => number.is_f64() && number.as_f64() == Some(0.0),
        Value::Array(items) => items.iter().any(json_holds_float_zero),
        Value::Object(fields) => fields.values().any(json_holds_float_zero),
        Value::Null | Value::Bool(_) | Value::String(_) => false,
    }
}

/// Whether any JSON value in `block`, its extras included, is a float zero of either sign.
pub(crate) fn holds_float_zero(block: &WireBlock) -> bool {
    fn extras(extras: &ProviderExtras) -> bool {
        extras
            .values()
            .flat_map(BTreeMap::values)
            .any(json_holds_float_zero)
    }
    fn media(media: &MediaBlock) -> bool {
        let MediaBlock { source, .. } = media;
        json_holds_float_zero(source)
    }
    fn opaque(opaque: &OpaqueBlock) -> bool {
        let OpaqueBlock {
            source, raw, arc, ..
        } = opaque;
        json_holds_float_zero(source)
            || json_holds_float_zero(raw)
            || arc.as_ref().is_some_and(json_holds_float_zero)
    }
    let kind = match block.kind() {
        BlockKind::Text { .. }
        | BlockKind::Reasoning { .. }
        | BlockKind::RedactedReasoning { .. } => false,
        BlockKind::ToolCall { input, .. } => json_holds_float_zero(input),
        BlockKind::ToolResult { output, .. } => {
            extras(&output.provider_extras)
                || match &output.kind {
                    OutputKind::Json { value: json } | OutputKind::ErrorJson { value: json } => {
                        json_holds_float_zero(json)
                    }
                    OutputKind::Text { .. }
                    | OutputKind::ErrorText { .. }
                    | OutputKind::ExecutionDenied { .. } => false,
                    OutputKind::Content { blocks } | OutputKind::ErrorContent { blocks } => {
                        blocks.iter().any(|result| {
                            extras(&result.provider_extras)
                                || match &result.kind {
                                    ResultBlockKind::Text { .. } => false,
                                    ResultBlockKind::Media { media: block } => media(block),
                                    ResultBlockKind::Opaque { opaque: block } => opaque(block),
                                }
                        })
                    }
                }
        }
        BlockKind::Media(block) => media(block),
        BlockKind::Opaque(block) => opaque(block),
    };
    kind || extras(&block.provider_extras)
}

fn alignment_candidate(
    block: &WireBlock,
    block_index: usize,
    meta: &BlockMeta,
    kind_matches: bool,
    decoded: &[WireBlock],
) -> bool {
    if kind_matches && decoded_matches(decoded, block, meta) {
        return true;
    }
    // A meta naming no decoded block pairs by position alone: an unchanged block index and a
    // matching kind, with no scan of nearby same-kind blocks.
    decoded.get(meta.block_index).is_none() && block_index == meta.block_index && kind_matches
}

/// Largest `blocks * metas` product the optimal alignment may allocate matrices for.
///
/// The score matrix costs one word per cell, so this caps it near 8 MiB; a single message
/// with thousands of blocks on both sides exceeds it and takes the greedy path, whose memory
/// is linear in the message's canonical bytes.
pub(crate) const MAX_ALIGNMENT_CELLS: usize = 1 << 20;

/// Aligns blocks with metadata without reordering either sequence, comparing each block with
/// the decoded block a meta names by canonical content.
///
/// Dynamic programming maximizes the number of matches. Each block and metadata row appears
/// in at most one pair. Time and memory are `O(blocks * metas)` up to
/// [`MAX_ALIGNMENT_CELLS`]; above that, a forward greedy walk pairs each block with the
/// first unpaired candidate, indexing the metas by their decoded blocks' canonical text, so
/// its memory is linear in the message's canonical bytes; it trades optimality for that bound.
pub(crate) fn match_block_metas<'a>(
    blocks: &[WireBlock],
    metas: &'a [BlockMeta],
    mut matches: impl FnMut(&WireBlock, &BlockMeta) -> bool,
    decoded: &'a [WireBlock],
) -> MatchedBlockMetas<'a> {
    let cells = blocks.len().saturating_mul(metas.len());
    let by_block = if cells > MAX_ALIGNMENT_CELLS {
        greedy_block_metas(blocks, metas, &mut matches, decoded)
    } else {
        optimal_block_metas(blocks, metas, &mut matches, decoded)
    };

    let native_parts = metas
        .iter()
        .filter_map(|meta| meta.native_index)
        .max()
        .map_or(0, |last| last + 1);
    let mut unretained_native_parts = vec![false; native_parts];
    for native_index in metas.iter().filter_map(|meta| meta.native_index) {
        unretained_native_parts[native_index] = true;
    }
    for native_index in by_block
        .iter()
        .filter_map(|meta| meta.and_then(|meta| meta.native_index))
    {
        unretained_native_parts[native_index] = false;
    }

    MatchedBlockMetas {
        by_block,
        unretained_native_parts,
        decoded,
    }
}

fn greedy_block_metas<'a>(
    blocks: &[WireBlock],
    metas: &'a [BlockMeta],
    matches: &mut impl FnMut(&WireBlock, &BlockMeta) -> bool,
    decoded: &[WireBlock],
) -> Vec<Option<&'a BlockMeta>> {
    // Every candidate pairing is reachable through one of two keys, so indexing the metas
    // once keeps each block to a few lookups instead of a rescan of the remaining suffix. A
    // meta with a decoded block is keyed by that block's canonical text, which a block's own
    // canonical text equals exactly when the two have the same content.
    let mut by_text: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut by_block_index: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (meta_index, meta) in metas.iter().enumerate() {
        match decoded.get(meta.block_index) {
            Some(decoded) => by_text
                .entry(canonical_text(decoded))
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
        let text = canonical_text(block);
        let mut kind_matches = |meta_index: usize| matches(block, &metas[meta_index]);
        let paired =
            first_at_or_after(by_text.get(&text), meta_cursor, &mut kind_matches).or_else(|| {
                first_at_or_after(
                    by_block_index.get(&block_index),
                    meta_cursor,
                    &mut kind_matches,
                )
            });
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
    decoded: &[WireBlock],
) -> Vec<Option<&'a BlockMeta>> {
    let (rows, columns) = (blocks.len(), metas.len());
    let mut candidates = vec![false; rows * columns];
    for (block_index, block) in blocks.iter().enumerate() {
        for (meta_index, meta) in metas.iter().enumerate() {
            let kind_matches = matches(block, meta);
            candidates[block_index * columns + meta_index] =
                alignment_candidate(block, block_index, meta, kind_matches, decoded);
        }
    }

    // The LCS-style walk preserves native order and avoids same-kind adjacency matching.
    let width = columns + 1;
    let mut scores = vec![0usize; (rows + 1) * width];
    for block_index in (0..rows).rev() {
        for meta_index in (0..columns).rev() {
            let cell = block_index * width + meta_index;
            let mut best = scores[cell + width].max(scores[cell + 1]);
            if candidates[block_index * columns + meta_index] {
                best = best.max(scores[cell + width + 1] + 1);
            }
            scores[cell] = best;
        }
    }

    let mut by_block = vec![None; rows];
    let (mut block_index, mut meta_index) = (0, 0);
    while block_index < rows && meta_index < columns {
        let cell = block_index * width + meta_index;
        if candidates[block_index * columns + meta_index]
            && scores[cell + width + 1] + 1 == scores[cell]
        {
            by_block[block_index] = Some(&metas[meta_index]);
            block_index += 1;
            meta_index += 1;
            continue;
        }
        if scores[cell + width] >= scores[cell + 1] {
            block_index += 1;
        } else {
            meta_index += 1;
        }
    }

    by_block
}

/// Returns up to `chars` lowercase hexadecimal characters from serialized JSON's SHA-256 digest.
///
/// The codecs digest only identities: a message id for a native message that carries none
/// (`opencode-hash-…`, `opencode-<role hash>`, Pi row ids) and a synthetic tool id
/// (`synth_tool_id`) for a call without one. These ids persist in session state,
/// so their spelling is stable. Block alignment compares canonical text and takes no digest.
///
/// Serialization failure hashes an empty byte sequence. Requests above 64 characters
/// return the full 64-character digest.
pub fn stable_hash_prefix(value: &Value, chars: usize) -> String {
    #[cfg(test)]
    count_digest();
    // The compact bytes stream into the digest, so hashing allocates nothing in proportion to
    // the value, whose escapes can make its encoding several times its retained size.
    let mut hasher = DigestWriter(Sha256::new());
    let digest = match serde_json::to_writer(&mut hasher, value) {
        Ok(()) => hasher.0.finalize(),
        Err(_) => Sha256::digest([]),
    };
    hex_prefix(&digest, chars.div_ceil(2))
        .chars()
        .take(chars)
        .collect()
}

#[cfg(test)]
thread_local! {
    /// SHA-256 digests the codecs have taken on this thread: identities through
    /// [`stable_hash_prefix`] and test fingerprints through [`decoded_block_fingerprint`],
    /// the two functions that digest anything.
    static CODEC_DIGESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn count_digest() {
    CODEC_DIGESTS.with(|digests| digests.set(digests.get() + 1));
}

/// The SHA-256 digests the codecs have taken on this thread so far.
#[cfg(test)]
pub(crate) fn codec_digests() -> usize {
    CODEC_DIGESTS.with(std::cell::Cell::get)
}

/// Feeds every byte written to it into its digest.
struct DigestWriter(Sha256);

impl std::io::Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn hex_prefix(bytes: &[u8], count: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(count * 2);
    for byte in bytes.iter().take(count) {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
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

    fn stable_hash(value: &Value) -> String {
        let digest = Sha256::digest(serde_json::to_vec(value).unwrap());
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn text_meta(block_index: usize) -> BlockMeta {
        BlockMeta {
            block_index,
            kind: "text".to_string(),
            native_index: Some(block_index),
            native_id: None,
            item_id: None,
        }
    }

    fn codec_namespaced(text: &str) -> WireBlock {
        let mut block = text_block(text);
        block
            .provider_extras
            .entry(BLOCK_IDENTITY_NAMESPACE.into())
            .or_default()
            .insert("blockIndex".into(), Value::from(7));
        block
    }

    /// The structural comparison is the fingerprint pin: two blocks have the same decoded
    /// content exactly when their canonical fingerprints agree, signed zeros and int/float
    /// variants included, and the codec namespace excluded.
    #[test]
    fn same_decoded_content_is_fingerprint_equality() {
        let call = |input: Value, provider_executed| {
            WireBlock::bare(BlockKind::ToolCall {
                id: "call".into(),
                name: "read".into(),
                input,
                provider_executed,
            })
        };
        let mut extras = text_block("a");
        extras
            .provider_extras
            .entry("pi".into())
            .or_default()
            .insert("textSignature".into(), Value::from("s"));
        let mut empty_namespace = text_block("a");
        empty_namespace
            .provider_extras
            .entry("pi".into())
            .or_default();
        let mut nested_zero_extras = text_block("a");
        nested_zero_extras
            .provider_extras
            .entry("opencode".into())
            .or_default()
            .insert("metadata".into(), serde_json::json!({ "z": [-0.0] }));
        let blocks = [
            text_block("a"),
            text_block("b"),
            codec_namespaced("a"),
            extras,
            empty_namespace,
            nested_zero_extras,
            call(serde_json::json!({ "n": 1 }), false),
            call(serde_json::json!({ "n": 1.0 }), false),
            call(serde_json::json!({ "n": 0.0 }), false),
            call(serde_json::json!({ "n": -0.0 }), false),
            call(serde_json::json!({ "n": { "deep": [0.0, 1] } }), false),
            call(serde_json::json!({ "n": { "deep": [-0.0, 1] } }), false),
            call(serde_json::json!({ "n": 1 }), true),
            call(serde_json::json!([1, 2]), false),
            call(serde_json::json!([2, 1]), false),
        ];
        // Literal anchors: a float zero and its opposite sign are other content, nested too.
        assert!(!same_decoded_content(&blocks[8], &blocks[9]));
        assert!(!same_decoded_content(&blocks[10], &blocks[11]));
        assert!(same_decoded_content(&blocks[0], &blocks[2]));
        for block in &blocks {
            for other in &blocks {
                assert_eq!(
                    same_decoded_content(block, other),
                    decoded_block_fingerprint(block) == decoded_block_fingerprint(other),
                    "{block:?} vs {other:?}"
                );
            }
        }
    }

    /// The fingerprint excludes exactly the codec namespace: the namespace never changes it,
    /// other provider namespaces always do.
    #[test]
    fn decoded_fingerprint_ignores_only_the_codec_namespace() {
        let block = text_block("hello");
        let bare = decoded_block_fingerprint(&block);
        assert_eq!(
            bare,
            stable_hash(&serde_json::to_value(&block).unwrap()),
            "typed serialization and canonical bytes agree for a plain block"
        );
        assert_eq!(decoded_block_fingerprint(&codec_namespaced("hello")), bare);

        let mut with_opencode = block.clone();
        with_opencode
            .provider_extras
            .entry("opencode".into())
            .or_default()
            .insert("metadata".into(), Value::from(1));
        let mut with_pi = block.clone();
        with_pi
            .provider_extras
            .entry("pi".into())
            .or_default()
            .insert("metadata".into(), Value::from(1));
        let opencode = decoded_block_fingerprint(&with_opencode);
        let pi = decoded_block_fingerprint(&with_pi);
        assert_ne!(opencode, bare);
        assert_ne!(pi, bare);
        assert_ne!(opencode, pi, "provider namespaces are significant");

        // A typed tool block omits its false flag, so it compares like an ingress block
        // whose emitter never sent the field.
        let typed = WireBlock::bare(BlockKind::ToolCall {
            id: "c".into(),
            name: "n".into(),
            input: Value::Null,
            provider_executed: false,
        });
        let ingress: WireBlock = serde_json::from_str(
            r#"{"kind":{"id":"c","input":null,"name":"n","type":"tool_call"}}"#,
        )
        .unwrap();
        assert_eq!(
            decoded_block_fingerprint(&typed),
            decoded_block_fingerprint(&ingress)
        );
        assert!(same_decoded_content(&typed, &ingress));
    }

    #[test]
    fn decoded_block_equality_ignores_only_the_codec_namespace() {
        let mut decoded = text_block("hello");
        decoded
            .provider_extras
            .entry("pi".into())
            .or_default()
            .insert("textSignature".into(), Value::from("sig"));
        let plain = decoded.clone();
        decoded
            .provider_extras
            .entry(BLOCK_IDENTITY_NAMESPACE.into())
            .or_default()
            .insert("blockIndex".into(), Value::from(0));
        assert!(equals_decoded_block(&plain, &decoded));

        let mut other_extras = plain.clone();
        other_extras
            .provider_extras
            .entry("pi".into())
            .or_default()
            .insert("textSignature".into(), Value::from("other"));
        assert!(!equals_decoded_block(&other_extras, &decoded));
        let mut no_extras = plain.clone();
        no_extras.provider_extras.clear();
        assert!(!equals_decoded_block(&no_extras, &decoded));
        assert!(!equals_decoded_block(&text_block("other"), &decoded));
    }

    /// Both regimes pair a served block with the meta whose decoded block has its content; the
    /// greedy walk above the cell bound pairs what the optimal walk pairs on an in-order
    /// window, including a float zero whose sign differs, which pairs with nothing.
    #[test]
    fn both_alignment_regimes_pair_by_canonical_content() {
        let count = 1_100;
        assert!(count * count > MAX_ALIGNMENT_CELLS);
        let call = |index: usize, zero: f64| {
            WireBlock::bare(BlockKind::ToolCall {
                id: format!("call-{index}"),
                name: "read".into(),
                input: serde_json::json!({ "offset": zero, "index": index }),
                provider_executed: false,
            })
        };
        let decoded: Vec<WireBlock> = (0..count).map(|index| call(index, 0.0)).collect();
        let metas: Vec<BlockMeta> = (0..count)
            .map(|index| BlockMeta {
                kind: "tool_call".into(),
                ..text_meta(index)
            })
            .collect();
        let mut served = decoded.clone();
        served[3] = call(3, -0.0);
        served.remove(10);
        let kind = |_: &WireBlock, meta: &BlockMeta| meta.kind == "tool_call";
        let greedy = match_block_metas(&served, &metas, kind, &decoded);
        let small = 40;
        let optimal = match_block_metas(&served[..small], &metas[..small + 1], kind, &decoded);
        for matched in [&greedy.by_block[..small], &optimal.by_block[..]] {
            let paired: Vec<Option<usize>> = matched
                .iter()
                .map(|meta| meta.map(|meta| meta.block_index))
                .collect();
            assert_eq!(paired[2], Some(2));
            assert_eq!(
                paired[3], None,
                "a float zero of the other sign is other content"
            );
            assert_eq!(paired[9], Some(9));
            assert_eq!(
                paired[10],
                Some(11),
                "a deleted block shifts its successors"
            );
        }
        assert!(greedy.block_is_unchanged(&served, 2, &metas[2]));
        assert!(!greedy.block_is_unchanged(&served, 3, &metas[3]));
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
        let matched = match_block_metas(&blocks, &metas, |_, meta| meta.kind == "text", &[]);
        for (index, meta) in matched.by_block.iter().enumerate() {
            assert_eq!(
                meta.map(|meta| meta.block_index),
                Some(index),
                "block {index} must pair with its positional meta"
            );
        }
        assert_eq!(matched.unretained_native_parts, vec![false; count]);
    }

    /// Every SHA-256 the codecs take runs in [`stable_hash_prefix`] or the test-only
    /// [`decoded_block_fingerprint`], the two functions [`codec_digests`] counts, so a digest
    /// added anywhere else in the codecs fails here before a counting test could miss it.
    #[test]
    fn every_codec_digest_runs_in_a_counted_function() {
        fn production(source: &str) -> &str {
            source
                .split_once("#[cfg(test)]\nmod tests {")
                .or_else(|| source.split_once("#[cfg(test)]\npub(crate) mod tests {"))
                .map_or(source, |(production, _)| production)
        }
        for source in [
            include_str!("json.rs"),
            include_str!("mod.rs"),
            include_str!("opencode.rs"),
            include_str!("pi.rs"),
        ] {
            let source = production(source);
            for digest in [
                "Sha256",
                "sha2",
                "Digest",
                "fingerprint(",
                "block_identity_digest",
            ] {
                assert!(!source.contains(digest), "{digest}");
            }
        }
        let sidecar = production(include_str!("sidecar.rs"));
        let digesting: Vec<&str> = sidecar
            .split("\n")
            .filter(|line| line.contains("Sha256::"))
            .collect();
        assert_eq!(
            digesting,
            [
                "    let digest = Sha256::digest(canonical_text(block).as_bytes());",
                "    let mut hasher = DigestWriter(Sha256::new());",
                "        Err(_) => Sha256::digest([]),",
            ]
        );
        for (function, count_first) in [
            (
                "pub(crate) fn decoded_block_fingerprint(",
                "count_digest();",
            ),
            ("pub fn stable_hash_prefix(", "count_digest();"),
        ] {
            let body = sidecar.split_once(function).unwrap().1;
            let counted = body.find(count_first).unwrap();
            assert!(counted < body.find("Sha256::").unwrap(), "{function}");
        }
    }

    #[test]
    fn hex_prefix_writes_two_lowercase_digits_per_byte() {
        let bytes = (0..=u8::MAX).collect::<Vec<_>>();
        let expected = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(hex_prefix(&bytes, bytes.len()), expected);
        assert_eq!(hex_prefix(&bytes, 3), "000102");
        assert_eq!(hex_prefix(&bytes, bytes.len() + 1), expected);
        assert_eq!(
            stable_hash_prefix(&Value::Null, 5),
            stable_hash(&Value::Null)[..5]
        );
    }

    #[test]
    fn in_place_edit_pairing_restores_equal_runs_and_leaves_ambiguous_runs() {
        let decoded = ["a", "b", "c"].map(text_block);
        let metas: Vec<BlockMeta> = (0..3).map(text_meta).collect();
        let text = |_: &WireBlock, meta: &BlockMeta| meta.kind == "text";
        let paired = |blocks: &[WireBlock]| {
            let mut matched = match_block_metas(blocks, &metas, text, &decoded);
            matched.pair_in_place_edits(blocks, &metas, text);
            let by_block = matched
                .by_block
                .iter()
                .map(|meta| meta.map(|meta| meta.block_index))
                .collect::<Vec<_>>();
            (
                by_block,
                matched.remove_unretained_native_parts(vec![0, 1, 2]),
            )
        };

        let edited = [text_block("a"), text_block("§1§ b"), text_block("c")];
        assert_eq!(
            paired(&edited),
            (vec![Some(0), Some(1), Some(2)], vec![0, 1, 2])
        );
        let leading_and_trailing = [text_block("§1§ a"), text_block("b"), text_block("§2§ c")];
        assert_eq!(
            paired(&leading_and_trailing),
            (vec![Some(0), Some(1), Some(2)], vec![0, 1, 2])
        );

        let deleted_and_edited = [text_block("§1§ b"), text_block("c")];
        assert_eq!(paired(&deleted_and_edited), (vec![None, Some(2)], vec![2]));
    }

    #[test]
    fn greedy_alignment_never_reuses_a_meta_and_preserves_order() {
        let blocks = vec![text_block("a"), text_block("b"), text_block("c")];
        // Only block 1 has a positional meta; blocks 0 and 2 have none.
        let metas = vec![text_meta(1)];
        let by_block = greedy_block_metas(&blocks, &metas, &mut |_, meta| meta.kind == "text", &[]);
        assert_eq!(by_block[0].map(|meta| meta.block_index), None);
        assert_eq!(by_block[1].map(|meta| meta.block_index), Some(1));
        assert_eq!(by_block[2].map(|meta| meta.block_index), None);
    }

    #[test]
    fn greedy_alignment_pairs_decoded_metas_through_the_content_index() {
        // Two metas describe decoded blocks that now sit after the positional ones; an
        // index-only walk would never reach them, and the content index must, in order and
        // without reusing a meta for the duplicate trailing copy.
        let decoded = [text_block("moved a"), text_block("moved b")];
        let mut metas = vec![text_meta(2), text_meta(3)];
        metas.extend([text_meta(0), text_meta(1)]);
        let mut blocks = vec![text_block("block 0"), text_block("block 1")];
        blocks.extend(decoded.iter().cloned());
        blocks.push(text_block("moved a"));

        let by_block = greedy_block_metas(
            &blocks,
            &metas,
            &mut |_, meta| meta.kind == "text",
            &decoded,
        );
        let paired: Vec<Option<usize>> = by_block
            .iter()
            .map(|meta| meta.map(|meta| meta.block_index))
            .collect();
        assert_eq!(paired, [None, None, Some(0), Some(1), None]);
    }
}
