//! This module estimates allocator-oriented retained sizes for memory-budgeted module holders.
//!
//! The estimates include inline collection elements, heap capacities, tree nodes, and `Arc` allocation headers.
//! Rust does not expose allocator size classes or `BTreeMap` node occupancy.
//! Cache admission and telemetry use the same routines, so their estimates cannot diverge.

use std::collections::{BTreeMap, HashMap};
use std::mem::size_of;

use memory_store::{
    BlockKind, HarnessMeta, MediaBlock, MessageOrigin, OpaqueBlock, OutputKind, ProviderExtras,
    ResultBlock, ResultBlockKind, ToolOutput, WireBlock, WireMessage,
};
use serde_json::Value;

/// Estimated strong and weak counters stored beside an `Arc` allocation.
pub(crate) const ARC_ALLOCATION_OVERHEAD_BYTES: usize = size_of::<usize>() * 2;
const BTREE_ENTRY_NODE_OVERHEAD_BYTES: usize = size_of::<usize>() * 3;

/// Estimates bytes retained by a cloned string, including its inline header.
pub(crate) fn cloned_string_retained_bytes(value: &str) -> usize {
    size_of::<String>().saturating_add(value.len())
}

/// Estimates `BTreeMap` allocation bytes for `len` entries.
///
/// The estimate charges each entry for inline key and value storage plus a
/// fixed three-word share of node metadata. Arithmetic saturates at `usize::MAX`.
pub(crate) fn btree_map_allocation_bytes<K, V>(len: usize) -> usize {
    len.saturating_mul(
        size_of::<K>()
            .saturating_add(size_of::<V>())
            .saturating_add(BTREE_ENTRY_NODE_OVERHEAD_BYTES),
    )
}

/// Estimates allocated hashbrown bucket bytes from admission capacity.
///
/// The estimate includes inline keys, inline values, and one control byte per
/// bucket. Arithmetic saturates at `usize::MAX`.
pub(crate) fn hash_map_allocation_bytes<K, V>(map: &HashMap<K, V>) -> usize {
    // hashbrown uses one control byte per bucket and admits seven entries per eight buckets.
    // The calculation converts admission capacity to bucket count because `capacity()` reports admission capacity.
    let buckets = map.capacity().saturating_mul(8).saturating_add(6) / 7;
    buckets.saturating_mul(
        size_of::<K>()
            .saturating_add(size_of::<V>())
            .saturating_add(1),
    )
}

/// Estimates total retained bytes for a JSON value, including its inline enum.
pub(crate) fn value_retained_bytes(value: &Value) -> usize {
    size_of::<Value>().saturating_add(value_heap_bytes(value))
}

/// Estimates heap bytes reachable from a JSON value.
///
/// String and vector branches charge capacity. Object branches charge entry
/// count because `serde_json::Map` does not expose tree-node capacity.
pub(crate) fn value_heap_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
        Value::String(value) => value.capacity(),
        Value::Array(values) => values
            .capacity()
            .saturating_mul(size_of::<Value>())
            .saturating_add(values.iter().map(value_heap_bytes).sum::<usize>()),
        Value::Object(values) => btree_map_allocation_bytes::<String, Value>(values.len())
            .saturating_add(
                values
                    .iter()
                    .map(|(key, value)| key.capacity().saturating_add(value_heap_bytes(value)))
                    .sum::<usize>(),
            ),
    }
}

/// Estimates heap bytes retained by provider namespaces, fields, and JSON values.
pub(crate) fn provider_extras_heap_bytes(extras: &ProviderExtras) -> usize {
    btree_map_allocation_bytes::<String, BTreeMap<String, Value>>(extras.len()).saturating_add(
        extras
            .iter()
            .map(|(namespace, fields)| {
                namespace
                    .capacity()
                    .saturating_add(btree_map_allocation_bytes::<String, Value>(fields.len()))
                    .saturating_add(
                        fields
                            .iter()
                            .map(|(key, value)| {
                                key.capacity().saturating_add(value_heap_bytes(value))
                            })
                            .sum::<usize>(),
                    )
            })
            .sum::<usize>(),
    )
}

fn optional_string_heap_bytes(value: Option<&String>) -> usize {
    value.map_or(0, String::capacity)
}

/// Estimates string-capacity bytes retained by optional message origin metadata.
pub(crate) fn origin_heap_bytes(origin: Option<&MessageOrigin>) -> usize {
    origin.map_or(0, |origin| {
        origin
            .provider
            .capacity()
            .saturating_add(origin.model.capacity())
            .saturating_add(origin.api.capacity())
    })
}

/// Estimates string-capacity bytes retained by harness metadata.
pub(crate) fn harness_meta_heap_bytes(meta: &HarnessMeta) -> usize {
    optional_string_heap_bytes(meta.harness_id.as_ref())
        .saturating_add(optional_string_heap_bytes(meta.finish.as_ref()))
}

fn opaque_block_heap_bytes(block: &OpaqueBlock) -> usize {
    value_heap_bytes(&block.source)
        .saturating_add(block.kind.capacity())
        .saturating_add(value_heap_bytes(&block.raw))
        .saturating_add(block.arc.as_ref().map_or(0, value_heap_bytes))
}

fn media_block_heap_bytes(block: &MediaBlock) -> usize {
    block
        .media_type
        .capacity()
        .saturating_add(optional_string_heap_bytes(block.filename.as_ref()))
        .saturating_add(value_heap_bytes(&block.source))
}

fn result_block_heap_bytes(block: &ResultBlock) -> usize {
    let kind_bytes = match &block.kind {
        ResultBlockKind::Text { text } => text.capacity(),
        ResultBlockKind::Media { media } => media_block_heap_bytes(media),
        ResultBlockKind::Opaque { opaque } => opaque_block_heap_bytes(opaque),
    };
    kind_bytes.saturating_add(provider_extras_heap_bytes(&block.provider_extras))
}

fn output_kind_heap_bytes(kind: &OutputKind) -> usize {
    match kind {
        OutputKind::Text { text } | OutputKind::ErrorText { text } => text.capacity(),
        OutputKind::Json { value } | OutputKind::ErrorJson { value } => value_heap_bytes(value),
        OutputKind::ExecutionDenied { reason } => optional_string_heap_bytes(reason.as_ref()),
        OutputKind::Content { blocks } | OutputKind::ErrorContent { blocks } => blocks
            .capacity()
            .saturating_mul(size_of::<ResultBlock>())
            .saturating_add(blocks.iter().map(result_block_heap_bytes).sum::<usize>()),
    }
}

fn tool_output_heap_bytes(output: &ToolOutput) -> usize {
    output_kind_heap_bytes(&output.kind)
        .saturating_add(provider_extras_heap_bytes(&output.provider_extras))
}

fn kind_heap_bytes(kind: &BlockKind) -> usize {
    match kind {
        BlockKind::Text { text } | BlockKind::RedactedReasoning { data: text } => text.capacity(),
        BlockKind::Reasoning { text, signature } => text
            .capacity()
            .saturating_add(optional_string_heap_bytes(signature.as_ref())),
        BlockKind::ToolCall {
            id, name, input, ..
        } => id
            .capacity()
            .saturating_add(name.capacity())
            .saturating_add(value_heap_bytes(input)),
        BlockKind::ToolResult {
            id,
            tool_name,
            output,
            ..
        } => id
            .capacity()
            .saturating_add(tool_name.capacity())
            .saturating_add(tool_output_heap_bytes(output)),
        BlockKind::Media(media) => media_block_heap_bytes(media),
        BlockKind::Opaque(opaque) => opaque_block_heap_bytes(opaque),
    }
}

/// Estimates total bytes retained by one wire block.
///
/// `size_of::<WireBlock>()` already covers the inline `Option<Value>` slot of the
/// retained original JSON, so the original contributes only its reachable heap bytes.
pub(crate) fn wire_block_retained_bytes(block: &WireBlock) -> usize {
    size_of::<WireBlock>()
        .saturating_add(kind_heap_bytes(block.kind()))
        .saturating_add(provider_extras_heap_bytes(&block.provider_extras))
        .saturating_add(block.original().map_or(0, value_heap_bytes))
}

/// Estimates total bytes retained by one wire message.
///
/// Block inline storage is charged through content capacity. Each block and the
/// message also charge their independently retained original JSON trees.
pub(crate) fn wire_message_retained_bytes(message: &WireMessage) -> usize {
    let blocks = message
        .content()
        .capacity()
        .saturating_mul(size_of::<WireBlock>())
        .saturating_add(
            message
                .content()
                .iter()
                .map(|block| {
                    wire_block_retained_bytes(block).saturating_sub(size_of::<WireBlock>())
                })
                .sum::<usize>(),
        );
    size_of::<WireMessage>()
        .saturating_add(message.role.capacity())
        .saturating_add(blocks)
        .saturating_add(origin_heap_bytes(message.origin.as_ref()))
        .saturating_add(provider_extras_heap_bytes(&message.provider_extras))
        .saturating_add(harness_meta_heap_bytes(&message.meta))
        // Message deserialization retains the complete original object independently of every block's original object.
        .saturating_add(message.original().map_or(0, value_heap_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_store::HarnessMeta;

    fn text_message(blocks: Vec<WireBlock>) -> WireMessage {
        WireMessage::from_parts(
            "user",
            blocks,
            None,
            ProviderExtras::new(),
            HarnessMeta::default(),
        )
    }

    #[test]
    fn original_json_is_charged_only_while_retained() {
        let bare = WireBlock::bare(BlockKind::Text {
            text: "a".repeat(16),
        });
        let mut json = serde_json::to_value(&bare).unwrap();
        json["future_field"] = Value::from("kept for pass-through");
        let deserialized: WireBlock = serde_json::from_value(json).unwrap();
        let original = deserialized.original().unwrap();
        assert_eq!(
            wire_block_retained_bytes(&deserialized) - wire_block_retained_bytes(&bare),
            value_heap_bytes(original)
        );

        let mut cleared = deserialized;
        cleared.mark_modified();
        assert_eq!(
            wire_block_retained_bytes(&cleared),
            wire_block_retained_bytes(&bare)
        );

        let constructed = text_message(vec![bare.clone()]);
        let reparsed: WireMessage =
            serde_json::from_value(serde_json::to_value(&constructed).unwrap()).unwrap();
        let message_original = reparsed.original().unwrap();
        let block_original = reparsed.content()[0].original().unwrap();
        assert_eq!(
            wire_message_retained_bytes(&reparsed) - wire_message_retained_bytes(&constructed),
            value_heap_bytes(message_original) + value_heap_bytes(block_original)
        );
    }

    #[test]
    fn message_accounting_charges_content_capacity_not_length() {
        let block = WireBlock::bare(BlockKind::Text { text: "a".into() });
        let mut exact = text_message(vec![block.clone()]);
        exact.content_mut().shrink_to_fit();
        let mut reserved = text_message(vec![block]);
        reserved.content_mut().reserve_exact(7);
        assert_eq!(exact.content().capacity(), 1);
        assert_eq!(reserved.content().capacity(), 8);

        assert_eq!(
            wire_message_retained_bytes(&reserved) - wire_message_retained_bytes(&exact),
            7 * size_of::<WireBlock>()
        );
    }
}
