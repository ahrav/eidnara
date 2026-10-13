//! Converts OpenCode MessageV2 JSON to CK wire messages and back.
//!
//! Decode sidecars retain native envelopes and block identities so unchanged
//! provider fields round-trip byte-for-byte at the JSON value level. Encoding
//! preserves message order and folds adjacent call/result pairs into OpenCode's
//! single tool-part representation.

use super::json::{media_kind, opaque_arc, set_string, set_value, string_field, synth_tool_id};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::injection::SYNTHETIC_TIMESTAMP;
use crate::wire::{
    BlockKind, HarnessMeta, IngressMessage, MediaBlock, MessageOrigin, OpaqueBlock, OutputKind,
    ProviderExtras, ResultBlock, ResultBlockKind, ToolOutput, WireBlock, WireMessage,
};

use super::sidecar::{
    BlockMeta, DecodeSidecar, DecodedHarnessMessages, ExtractedBoundary, HarnessMessageMeta,
    is_synthetic_part, match_block_metas, meta_for_ck, stable_hash_prefix,
};

pub type MessageV2Json = Value;

const HARNESS: &str = "opencode";

#[cfg(test)]
pub(crate) fn decode_opencode(messages: &[MessageV2Json]) -> DecodedHarnessMessages {
    let shared = messages.iter().cloned().map(Arc::new).collect::<Vec<_>>();
    decode_opencode_shared(&shared)
}

/// Decodes shared messages.
///
/// Each envelope is retained in `HarnessMessageMeta::raw` through an `Arc`
/// clone, so callers that already share values pay no deep copy. A message's
/// ordinal is its window position, `index + 1` with saturating arithmetic. The last compaction part becomes the extracted boundary and is
/// omitted from CK content. Unknown parts remain opaque. A creation time no `i64` holds is
/// left unset.
#[cfg(test)]
pub(crate) fn decode_opencode_shared(messages: &[Arc<Value>]) -> DecodedHarnessMessages {
    decode_window_rows(messages, Decode::MessagesAndSidecar)
}

/// The sidecar native attach encodes against: each message's envelope, block metas, and decoded
/// blocks, with no CK messages built.
pub(crate) fn decode_opencode_sidecar(messages: &[Arc<Value>]) -> DecodeSidecar {
    decode_window_rows(messages, Decode::Sidecar).sidecar
}

fn decode_window_rows(messages: &[Arc<Value>], decode: Decode) -> DecodedHarnessMessages {
    let rows = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (window_position(index), message));
    let Ok(decoded) = decode_rows(rows, decode, |time| Ok::<_, Infallible>(time.as_i64()));
    decoded
}

/// What a decode builds: the CK window alone, the sidecar alone, or both.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Decode {
    Window,
    Sidecar,
    #[cfg(test)]
    MessagesAndSidecar,
}

/// Derives the CK window the OpenCode plugin encodes from the native window `messages`.
///
/// A compaction summary row, one whose `info` holds `summary: true` and `finish: "stop"`,
/// leaves the window, and every kept message keeps its unfiltered window position, which names
/// its synthetic tool calls as the plugin's encoder names them. The derived messages carry no
/// ordinal and no block identity, as the plugin sends them, and no sidecar is built. A creation
/// time the request decode would refuse is refused with the same error, which the caller
/// answers as `bad_request` like the request decode.
pub(crate) fn derive_opencode_window(
    messages: &[Arc<Value>],
) -> Result<Vec<IngressMessage>, serde_json::Error> {
    let rows = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| !is_compaction_summary(message))
        .map(|(index, message)| (window_position(index), message));
    let decoded = decode_rows(rows, Decode::Window, |time| {
        <i64 as serde::Deserialize>::deserialize(time).map(Some)
    })?;
    Ok(decoded.messages)
}

fn window_position(index: usize) -> u64 {
    (index as u64).saturating_add(1)
}

fn is_compaction_summary(message: &Value) -> bool {
    message
        .get("info")
        .and_then(Value::as_object)
        .is_some_and(|info| {
            info.get("summary") == Some(&Value::Bool(true))
                && info.get("finish").and_then(Value::as_str) == Some("stop")
        })
}

/// The envelope a message's fields are read from: its `info` object, else the message itself.
fn message_info(message: &Value) -> &Value {
    match message.get("info") {
        Some(info @ (Value::Object(_) | Value::Array(_))) => info,
        _ => message,
    }
}

/// The first numeric creation time among `time.created`, `time_created`, and `timeCreated`.
fn created_at<E>(
    info: &Value,
    read: impl FnOnce(&Value) -> Result<Option<i64>, E>,
) -> Result<Option<i64>, E> {
    let time = info
        .get("time")
        .filter(|time| time.is_object() || time.is_array());
    let found = [
        time.and_then(|time| time.get("created")),
        info.get("time_created"),
        info.get("timeCreated"),
    ]
    .into_iter()
    .flatten()
    .find(|value| value.is_number());
    found.map_or(Ok(None), read)
}

/// Decodes `(window position, message)` rows. With a sidecar, every message is remembered in it
/// with its block metas and decoded blocks, and a message's ordinal is its position; the CK
/// window alone carries no ordinal, as the plugin sends it.
fn decode_rows<'a, E>(
    rows: impl Iterator<Item = (u64, &'a Arc<Value>)>,
    decode: Decode,
    mut read_time: impl FnMut(&Value) -> Result<Option<i64>, E>,
) -> Result<DecodedHarnessMessages, E> {
    let mut sidecar = DecodeSidecar::new(HARNESS);

    let mut decoded = Vec::with_capacity(rows.size_hint().0);
    let mut boundary = None;
    // A call id the window already carries contributes only its result, as the plugin encodes it.
    let mut emitted_calls = HashSet::new();
    let retain_sidecar = decode != Decode::Window;

    for (ordinal, raw_message) in rows {
        let info = message_info(raw_message);
        let stable_key = string_field(info, "id")
            .or_else(|| string_field(raw_message, "id"))
            .unwrap_or_else(|| format!("opencode-hash-{}", stable_hash_prefix(raw_message, 24)));
        // Each call decodes the whole array with no prior sidecar, so the stable key is the mid.
        let mid = stable_key.clone();

        let role = string_field(info, "role")
            .or_else(|| string_field(raw_message, "role"))
            .unwrap_or_else(|| "user".to_string());
        let origin = opencode_origin(info).or_else(|| opencode_origin(raw_message));
        let created_at_ms = created_at(info, &mut read_time)?;
        let parts = raw_message
            .get("parts")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();

        let mut blocks = Blocks::new(retain_sidecar);

        for (part_index, part) in parts.iter().enumerate() {
            let part_type = part
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            match part_type {
                "text" if part.get("ignored") != Some(&Value::Bool(true)) => {
                    let text = string_field(part, "text").unwrap_or_default();
                    let block = block_with_extras(BlockKind::Text { text }, part);
                    blocks.push(block, part_index, part, "text");
                }
                "reasoning" | "thinking" => {
                    let text = string_field(part, "text")
                        .or_else(|| string_field(part, "thinking"))
                        .unwrap_or_default();
                    let redacted = text
                        .is_empty()
                        .then(|| redacted_reasoning_data(part))
                        .flatten();
                    let kind = match redacted {
                        Some(data) => BlockKind::RedactedReasoning { data },
                        None => BlockKind::Reasoning {
                            text,
                            signature: part
                                .get("metadata")
                                .and_then(find_signature)
                                .or_else(|| string_field(part, "signature")),
                        },
                    };
                    let block = block_with_extras(kind, part);
                    blocks.push(block, part_index, part, "reasoning");
                }
                "redacted_thinking" => {
                    let data = string_field(part, "data")
                        .or_else(|| string_field(part, "redacted"))
                        .unwrap_or_default();
                    let block = block_with_extras(BlockKind::RedactedReasoning { data }, part);
                    blocks.push(block, part_index, part, "redacted_reasoning");
                }
                "tool" => {
                    decode_tool_part(ordinal, part_index, part, &mut emitted_calls, &mut blocks);
                }
                "file" | "image" => {
                    let media = media_from_part(part);
                    let block = WireBlock::bare(BlockKind::Media(media));
                    blocks.push(block, part_index, part, "file");
                }
                "step-start" | "step-finish" | "subtask" => {
                    let block = opaque_block(part_type, part.clone(), None);
                    blocks.push(block, part_index, part, part_type);
                }
                "compaction" => {
                    boundary = Some(ExtractedBoundary {
                        harness: HARNESS.to_string(),
                        message_id: mid.clone(),
                        ordinal,
                        part_index: Some(part_index),
                        entry_id: None,
                        raw: part.clone(),
                    });
                }
                "snapshot" | "patch" | "agent" | "retry" => {}
                // An ignored text part stays in the window as an opaque `text` block.
                _ => {
                    let block = opaque_block(part_type, part.clone(), opaque_arc(part));
                    blocks.push(block, part_index, part, part_type);
                }
            }
        }

        let synthetic = is_synthetic_message(parts);
        let (content, block_metas) = blocks.finish();
        let recorded_ordinal = retain_sidecar.then_some(ordinal);
        let mut ck = WireMessage::from_parts(
            role.clone(),
            content,
            origin,
            ProviderExtras::new(),
            HarnessMeta {
                harness_id: Some(mid.clone()),
                ordinal: recorded_ordinal,
                synthetic,
                summary: info.get("summary") == Some(&Value::Bool(true)),
                errored: info.get("error").is_some_and(|error| !error.is_null()),
                finish: string_field(info, "finish"),
                created_at_ms,
            },
        );
        let decoded_blocks = match decode {
            Decode::Window => Vec::new(),
            Decode::Sidecar => std::mem::take(ck.content_mut()),
            #[cfg(test)]
            Decode::MessagesAndSidecar => ck.content().to_vec(),
        };
        if decode != Decode::Sidecar {
            decoded.push(IngressMessage {
                mid: mid.clone(),
                ordinal: recorded_ordinal.unwrap_or_default(),
                ck,
            });
        }
        if let Some(block_metas) = block_metas {
            sidecar.remember_message(
                mid.clone(),
                HarnessMessageMeta {
                    mid,
                    ordinal,
                    role,
                    raw: Arc::clone(raw_message),
                    stable_key: Some(stable_key),
                    blocks: block_metas,
                    decoded: decoded_blocks,
                },
            );
        }
    }

    Ok(DecodedHarnessMessages {
        messages: decoded,
        boundary,
        sidecar,
    })
}

/// Encodes CK messages and reuses retained native envelopes when available.
///
/// `mutation_exempt_mid` selects one retained message for exact raw replay.
/// In debug builds, encoding panics if two emitted tool parts share a call ID.
pub fn encode_opencode(
    messages: &[WireMessage],
    sidecar: &DecodeSidecar,
    mutation_exempt_mid: Option<&str>,
) -> Vec<MessageV2Json> {
    owned(match mutation_exempt_mid {
        Some(mid) => encode_opencode_impl(messages, sidecar, None, false, &[mid]),
        None => encode_opencode_impl(messages, sidecar, None, false, &[]),
    })
}

/// Encodes CK messages with a session ID for new synthetic user messages.
///
/// `session_id` applies only to newly created synthetic user messages.
/// Retained raw values preserve provider fields that CK does not model.
/// The decoder retains compaction parts to replay untouched ingress.
pub fn encode_opencode_with_session(
    messages: &[WireMessage],
    sidecar: &DecodeSidecar,
    session_id: Option<&str>,
    mutation_exempt_mid: Option<&str>,
) -> Vec<MessageV2Json> {
    owned(match mutation_exempt_mid {
        Some(mid) => encode_opencode_impl(messages, sidecar, session_id, true, &[mid]),
        None => encode_opencode_impl(messages, sidecar, session_id, true, &[]),
    })
}

#[cfg(test)]
pub(crate) fn encode_opencode_with_session_exemptions(
    messages: &[WireMessage],
    sidecar: &DecodeSidecar,
    session_id: Option<&str>,
    mutation_exempt_mids: &[&str],
) -> Vec<MessageV2Json> {
    owned(encode_opencode_shared(
        messages,
        sidecar,
        session_id,
        mutation_exempt_mids,
    ))
}

/// Encodes as [`encode_opencode_with_session_exemptions`] does, sharing each envelope the
/// encode leaves unchanged: a message that replays its retained envelope is that `Arc`.
pub(crate) fn encode_opencode_shared(
    messages: &[WireMessage],
    sidecar: &DecodeSidecar,
    session_id: Option<&str>,
    mutation_exempt_mids: &[&str],
) -> Vec<Arc<MessageV2Json>> {
    encode_opencode_impl(messages, sidecar, session_id, true, mutation_exempt_mids)
}

fn encode_opencode_impl(
    messages: &[WireMessage],
    sidecar: &DecodeSidecar,
    session_id: Option<&str>,
    preserve_compaction: bool,
    mutation_exempt_mids: &[&str],
) -> Vec<Arc<MessageV2Json>> {
    let mut encoded = Vec::with_capacity(messages.len());
    let mut index = 0;
    while index < messages.len() {
        if let Some(next) = messages.get(index + 1) {
            if let Some(part) = render_synthetic_todo_pair(&messages[index], next) {
                let info = synthetic_message_info(&messages[index], session_id);
                encoded.push(Arc::new(json!({
                    "info": info,
                    "parts": [part],
                })));
                index += 2;
                continue;
            }
            let call_is_fresh = meta_for_ck(sidecar, &messages[index], index).is_none();
            let result_is_fresh = meta_for_ck(sidecar, next, index + 1).is_none();
            if call_is_fresh
                && result_is_fresh
                && let Some(part) = render_adjacent_tool_pair(&messages[index], next)
            {
                let mut message = encode_new_message(&messages[index], session_id);
                set_value(&mut message, "parts", Value::Array(vec![part]));
                encoded.push(Arc::new(message));
                index += 2;
                continue;
            }
        }
        let msg = &messages[index];
        // Decoded messages retain their harness IDs for metadata rebinding.
        // A positional synthetic fallback may attach an input nudge envelope to a fresh module-authored message.
        // A positional synthetic fallback may attach a native envelope to a fresh module-authored m0/m1 message.
        let meta = meta_for_ck(sidecar, msg, index);
        encoded.push(match meta {
            Some(meta) if mutation_exempt_mids.contains(&meta.mid.as_str()) => {
                Arc::clone(&meta.raw)
            }
            Some(meta) => encode_with_meta(msg, meta, preserve_compaction),
            None => Arc::new(encode_new_message(msg, session_id)),
        });
        index += 1;
    }
    assert_unique_tool_use_ids(encoded.iter().map(Arc::as_ref));
    encoded
}

/// The owned values of an encode, for callers that edit them.
fn owned(encoded: Vec<Arc<MessageV2Json>>) -> Vec<MessageV2Json> {
    encoded.into_iter().map(Arc::unwrap_or_clone).collect()
}

fn duplicate_tool_use_locations<'a>(
    messages: impl IntoIterator<Item = &'a MessageV2Json>,
) -> Vec<(String, usize, usize)> {
    let mut seen = std::collections::HashSet::new();
    let mut duplicates = Vec::new();
    for (message_index, message) in messages.into_iter().enumerate() {
        let Some(parts) = message.get("parts").and_then(Value::as_array) else {
            continue;
        };
        for (part_index, part) in parts.iter().enumerate() {
            if part.get("type").and_then(Value::as_str) != Some("tool") {
                continue;
            }
            let Some(call_id) = part.get("callID").and_then(Value::as_str) else {
                continue;
            };
            if !seen.insert(call_id.to_string()) {
                duplicates.push((call_id.to_string(), message_index, part_index));
            }
        }
    }
    duplicates
}

fn assert_unique_tool_use_ids<'a>(messages: impl IntoIterator<Item = &'a MessageV2Json>) {
    let duplicates = duplicate_tool_use_locations(messages);
    debug_assert!(
        duplicates.is_empty(),
        "OpenCode serialization produced duplicate tool_use ids: {duplicates:?}"
    );
}

fn decode_tool_part<'a>(
    ordinal: u64,
    part_index: usize,
    part: &'a Value,
    emitted_calls: &mut HashSet<Cow<'a, str>>,
    blocks: &mut Blocks,
) {
    let tool_name = tool_name(part);
    let input = part
        .get("state")
        .and_then(|state| state.get("input"))
        .or_else(|| part.get("input"))
        .or_else(|| part.get("args"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let id = ["callID", "callId", "id"]
        .into_iter()
        .find_map(|key| part.get(key).and_then(Value::as_str))
        .map_or_else(
            || Cow::Owned(synth_tool_id(ordinal, part_index, &tool_name, &input)),
            Cow::Borrowed,
        );
    let provider_executed = part
        .get("metadata")
        .and_then(|m| m.get("providerExecuted"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if emitted_calls.insert(id.clone()) {
        let call_block = WireBlock::bare(BlockKind::ToolCall {
            id: id.to_string(),
            name: tool_name.clone(),
            input,
            provider_executed,
        });
        blocks.push_with_native_id(call_block, part_index, "tool_call", Some(&id));
    }

    let status = tool_status(part);
    if matches!(status.as_deref(), Some("completed" | "error")) {
        let output_text = part
            .get("state")
            .and_then(|state| state.get("output").or_else(|| state.get("error")))
            .or_else(|| part.get("output").or_else(|| part.get("error")))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let output = tool_output_from_part(part, status.as_deref() == Some("error"), output_text);
        let result_block = WireBlock::bare(BlockKind::ToolResult {
            id: id.to_string(),
            tool_name,
            output,
            provider_executed,
        });
        blocks.push_with_native_id(result_block, part_index, "tool_result", Some(&id));
    }
}

/// One message's decoded blocks and, when the sidecar is retained, each block's meta.
struct Blocks {
    content: Vec<WireBlock>,
    metas: Option<Vec<BlockMeta>>,
}

impl Blocks {
    fn new(retain_sidecar: bool) -> Self {
        Self {
            content: Vec::new(),
            metas: retain_sidecar.then(Vec::new),
        }
    }

    /// Appends `block`, decoded from the native part `raw` at `part_index`; with a retained
    /// sidecar its meta is recorded under `kind`, named by the part's own `id` or `callID`.
    fn push(&mut self, block: WireBlock, part_index: usize, raw: &Value, kind: &str) {
        let native_id = ["id", "callID"]
            .into_iter()
            .find_map(|key| raw.get(key).and_then(Value::as_str));
        self.push_with_native_id(block, part_index, kind, native_id);
    }

    /// As [`Self::push`], with the meta named by `native_id`.
    fn push_with_native_id(
        &mut self,
        block: WireBlock,
        part_index: usize,
        kind: &str,
        native_id: Option<&str>,
    ) {
        if let Some(metas) = &mut self.metas {
            metas.push(BlockMeta {
                block_index: self.content.len(),
                kind: kind.to_string(),
                native_index: Some(part_index),
                native_id: native_id.map(str::to_string),
                item_id: None,
            });
        }
        self.content.push(block);
    }

    fn finish(self) -> (Vec<WireBlock>, Option<Vec<BlockMeta>>) {
        (self.content, self.metas)
    }
}

/// A block carrying the part's `metadata` and `cache_control` under the `opencode` namespace.
fn block_with_extras(kind: BlockKind, part: &Value) -> WireBlock {
    let mut namespace = BTreeMap::new();
    for key in ["metadata", "cache_control"] {
        if let Some(value) = part.get(key) {
            namespace.insert(key.to_string(), value.clone());
        }
    }
    if namespace.is_empty() {
        WireBlock::bare(kind)
    } else {
        WireBlock::with_provider_extras(
            kind,
            ProviderExtras::from([(HARNESS.to_string(), namespace)]),
        )
    }
}

fn opencode_origin(value: &Value) -> Option<MessageOrigin> {
    let model = value.get("model").unwrap_or(value);
    let provider = string_field(model, "providerID")
        .or_else(|| string_field(model, "provider"))
        .or_else(|| string_field(value, "providerID"))
        .or_else(|| string_field(value, "provider"))?;
    let model_id = string_field(model, "modelID")
        .or_else(|| string_field(model, "model"))
        .or_else(|| string_field(value, "modelID"))
        .or_else(|| string_field(value, "model"))?;
    Some(MessageOrigin {
        api: provider.clone(),
        provider,
        model: model_id,
    })
}

fn media_from_part(part: &Value) -> MediaBlock {
    let media_type = string_field(part, "mime")
        .or_else(|| string_field(part, "mimeType"))
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let filename = string_field(part, "filename").or_else(|| string_field(part, "name"));
    let source = media_source(part, &media_type);
    MediaBlock {
        kind: media_kind(&media_type),
        media_type,
        filename,
        source,
    }
}

fn media_source(part: &Value, media_type: &str) -> Value {
    if let Some(data) = string_field(part, "data") {
        return json!({ "type": "data_base64", "data": data });
    }
    if let Some(url) = string_field(part, "url") {
        let prefix = format!("data:{media_type};base64,");
        if let Some(data) = url.strip_prefix(&prefix) {
            return json!({ "type": "data_base64", "data": data });
        }
        return json!({ "type": "url", "url": url });
    }
    json!({ "type": "opaque", "raw": part })
}

fn tool_output_from_part(part: &Value, is_error: bool, output_text: String) -> ToolOutput {
    let attachments = part
        .get("state")
        .and_then(|state| state.get("attachments"))
        .or_else(|| part.get("attachments"))
        .and_then(Value::as_array);
    let Some(attachments) = attachments else {
        return if is_error {
            ToolOutput::bare(OutputKind::ErrorText { text: output_text })
        } else {
            ToolOutput::bare(OutputKind::Text { text: output_text })
        };
    };

    let mut blocks = Vec::new();
    if !output_text.is_empty() {
        blocks.push(ResultBlock {
            kind: ResultBlockKind::Text { text: output_text },
            provider_extras: ProviderExtras::new(),
        });
    }
    for attachment in attachments {
        if !attachment.is_object() {
            continue;
        }
        let mut provider_extras = ProviderExtras::new();
        provider_extras
            .entry(HARNESS.to_string())
            .or_default()
            .insert("rawAttachment".to_string(), attachment.clone());
        let has_media_shape = attachment.get("mime").is_some()
            || attachment.get("mimeType").is_some()
            || matches!(
                attachment.get("type").and_then(Value::as_str),
                Some("file" | "image")
            );
        let kind = if has_media_shape {
            ResultBlockKind::Media {
                media: media_from_part(attachment),
            }
        } else {
            ResultBlockKind::Opaque {
                opaque: OpaqueBlock {
                    source: json!({ "type": "harness", "harness": HARNESS }),
                    kind: string_field(attachment, "type")
                        .unwrap_or_else(|| "attachment".to_string()),
                    raw: attachment.clone(),
                    arc: None,
                },
            }
        };
        blocks.push(ResultBlock {
            kind,
            provider_extras,
        });
    }
    ToolOutput::bare(if is_error {
        OutputKind::ErrorContent { blocks }
    } else {
        OutputKind::Content { blocks }
    })
}

/// Encodes `msg` over its retained envelope. A message whose every block is its decoded block,
/// in order, under an envelope that already names it with its role, is the envelope itself:
/// the retained `Arc`, with no copy.
fn encode_with_meta(
    msg: &WireMessage,
    meta: &HarnessMessageMeta,
    preserve_compaction: bool,
) -> Arc<Value> {
    let matched_metas = align_with_meta(msg, meta);
    if envelope_is_unchanged(msg, meta, &matched_metas, preserve_compaction) {
        return Arc::clone(&meta.raw);
    }
    render_with_meta(msg, meta, &matched_metas, preserve_compaction)
}

/// Pairs `msg`'s blocks with `meta`'s block metas by canonical content, then pairs the runs
/// of edited blocks between matches in place.
fn align_with_meta<'a>(
    msg: &WireMessage,
    meta: &'a HarnessMessageMeta,
) -> super::sidecar::MatchedBlockMetas<'a> {
    let mut matched_metas = match_block_metas(
        msg.content(),
        &meta.blocks,
        block_matches_meta,
        &meta.decoded,
    );
    matched_metas.pair_in_place_edits(msg.content(), &meta.blocks, block_matches_meta);
    matched_metas
}

/// Renders `msg` onto its retained envelope part by part: unchanged blocks keep their native
/// parts, edited blocks update theirs, and fresh blocks append. An envelope equal to the
/// retained one is the retained `Arc`.
fn render_with_meta(
    msg: &WireMessage,
    meta: &HarnessMessageMeta,
    matched_metas: &super::sidecar::MatchedBlockMetas<'_>,
    preserve_compaction: bool,
) -> Arc<Value> {
    let mut raw = meta.raw.as_ref().clone();
    let mut parts = raw
        .get_mut("parts")
        .and_then(Value::as_array_mut)
        .map(std::mem::take)
        .unwrap_or_default();

    let mut block_index = 0;
    while block_index < msg.content().len() {
        let block = &msg.content()[block_index];
        let block_meta = matched_metas.by_block[block_index];

        if let BlockKind::ToolCall { id, .. } = block.kind()
            && let Some(result) = msg.content().get(block_index + 1)
            && let BlockKind::ToolResult { id: result_id, .. } = result.kind()
            && id == result_id
        {
            let result_meta = matched_metas.by_block[block_index + 1];
            let call_native_index = block_meta.and_then(|meta| meta.native_index);
            let result_native_index = result_meta.and_then(|meta| meta.native_index);
            let shared_native_index = match (call_native_index, result_native_index) {
                (Some(call), Some(result)) if call == result => Some(call),
                (Some(call), None) => Some(call),
                (None, Some(result)) => Some(result),
                _ => None,
            };

            if let Some(part) = shared_native_index.and_then(|index| parts.get_mut(index)) {
                for (arc_index, arc_meta) in
                    [(block_index, block_meta), (block_index + 1, result_meta)]
                {
                    if !arc_meta.is_some_and(|meta| {
                        matched_metas.block_is_unchanged(msg.content(), arc_index, meta)
                    }) {
                        update_part_from_block(part, &msg.content()[arc_index]);
                    }
                }
                block_index += 2;
                continue;
            }
            if call_native_index.is_none() && result_native_index.is_none() {
                // OpenCode represents a completed invocation as one part; CK represents it as adjacent call and result blocks.
                // The paired call and result blocks must retain one `callID`.
                // The paired blocks must retain one `callID` even when no renderer-transition marker exists.
                // OpenCode matches call and result shells by their shared `callID`.
                parts.push(render_tool_pair_as_part(block, result));
                block_index += 2;
                continue;
            }
        }

        if let Some(part_index) = block_meta.and_then(|block_meta| block_meta.native_index)
            && let Some(part) = parts.get_mut(part_index)
        {
            if block_meta.is_some_and(|meta| {
                matched_metas.block_is_unchanged(msg.content(), block_index, meta)
            }) {
                block_index += 1;
                continue;
            }
            // The encoder preserves matched native reasoning parts exactly because provider signatures may require their original bytes.
            // The converter applies updates only to separately mapped sibling parts.
            if !matches!(
                block.kind(),
                BlockKind::Reasoning { .. } | BlockKind::RedactedReasoning { .. }
            ) {
                update_part_from_block(part, block);
            }
            block_index += 1;
            continue;
        }
        parts.push(render_block_as_part(block));
        block_index += 1;
    }

    parts = matched_metas.remove_unretained_native_parts(parts);
    if !preserve_compaction {
        parts.retain(|part| part.get("type").and_then(Value::as_str) != Some("compaction"));
    }

    if let Some(obj) = raw.as_object_mut() {
        obj.insert("parts".to_string(), Value::Array(parts));
        let info = obj.entry("info").or_insert_with(|| json!({}));
        if let Some(info_obj) = info.as_object_mut() {
            info_obj
                .entry("id".to_string())
                .or_insert_with(|| Value::String(meta.mid.clone()));
            info_obj.insert("role".to_string(), Value::String(msg.role.clone()));
        }
    } else {
        raw = json!({
            "info": { "id": meta.mid, "role": msg.role },
            "parts": parts,
        });
    }
    // An equal envelope is the retained one. JSON value equality treats `0.0` and `-0.0` as
    // equal although they serialize apart, so returning `meta.raw` also keeps the original
    // float-zero signs.
    if raw == *meta.raw {
        Arc::clone(&meta.raw)
    } else {
        Arc::new(raw)
    }
}

/// Whether encoding `msg` over `meta` would leave its envelope as retained: every served block
/// paired, in order, with the meta at its own position and equal to its decoded block, every
/// meta paired, compaction parts kept, a `parts` array, and an `info` that already holds the
/// message's id and role.
fn envelope_is_unchanged(
    msg: &WireMessage,
    meta: &HarnessMessageMeta,
    matched_metas: &super::sidecar::MatchedBlockMetas<'_>,
    preserve_compaction: bool,
) -> bool {
    let blocks = msg.content();
    let info = meta.raw.get("info");
    preserve_compaction
        && blocks.len() == meta.blocks.len()
        && meta.raw.get("parts").is_some_and(Value::is_array)
        && info.and_then(|info| info.get("id")).is_some()
        && info
            .and_then(|info| info.get("role"))
            .and_then(Value::as_str)
            == Some(&msg.role)
        && (0..blocks.len()).all(|index| {
            matched_metas.by_block[index].is_some_and(|paired| {
                std::ptr::eq(paired, &meta.blocks[index])
                    && matched_metas.block_is_unchanged(blocks, index, paired)
            })
        })
}

fn block_matches_meta(block: &WireBlock, meta: &BlockMeta) -> bool {
    match block.kind() {
        BlockKind::Text { text } => {
            meta.kind == "text"
                || (text.is_empty()
                    && matches!(meta.kind.as_str(), "reasoning" | "redacted_reasoning"))
        }
        BlockKind::Reasoning { .. } => meta.kind == "reasoning",
        BlockKind::RedactedReasoning { .. } => {
            matches!(meta.kind.as_str(), "reasoning" | "redacted_reasoning")
        }
        BlockKind::ToolCall { id, .. } => {
            meta.kind == "tool_call" && meta.native_id.as_deref().is_none_or(|native| native == id)
        }
        BlockKind::ToolResult { id, .. } => {
            meta.kind == "tool_result"
                && meta.native_id.as_deref().is_none_or(|native| native == id)
        }
        BlockKind::Media(_) => meta.kind == "file",
        BlockKind::Opaque(opaque) => meta.kind == opaque.kind,
    }
}

fn update_part_from_block(part: &mut Value, block: &WireBlock) {
    match block.kind() {
        BlockKind::Text { text } => {
            set_string(part, "type", "text");
            set_string(part, "text", text);
            if let Some(metadata) = block
                .provider_extras
                .get(HARNESS)
                .and_then(|ns| ns.get("metadata"))
            {
                set_value(part, "metadata", metadata.clone());
            }
        }
        BlockKind::Reasoning { text, signature } => {
            set_string(part, "type", "reasoning");
            set_string(part, "text", text);
            if part.get("metadata").is_none()
                && let Some(signature) = signature
            {
                set_value(part, "metadata", json!({ "signature": signature }));
            }
        }
        BlockKind::RedactedReasoning { data } => {
            set_string(part, "type", "reasoning");
            set_string(part, "text", "");
            if part.get("metadata").is_none() {
                set_value(part, "metadata", json!({ "redacted": data }));
            }
        }
        BlockKind::ToolCall {
            id,
            name,
            input,
            provider_executed,
        } => {
            set_string(part, "type", "tool");
            set_string(part, "callID", id);
            set_string(part, "tool", name);
            set_nested_value(part, "state", "input", input.clone());
            if *provider_executed {
                set_nested_value(part, "metadata", "providerExecuted", Value::Bool(true));
            }
        }
        BlockKind::ToolResult {
            id,
            tool_name,
            output,
            ..
        } => {
            set_string(part, "type", "tool");
            set_string(part, "callID", id);
            set_string(part, "tool", tool_name);
            apply_tool_output_to_part(part, output);
        }
        BlockKind::Media(media) => {
            *part = render_media_part(media);
        }
        BlockKind::Opaque(opaque) => {
            *part = opaque.raw.clone();
        }
    }
}

fn render_adjacent_tool_pair(call: &WireMessage, result: &WireMessage) -> Option<Value> {
    if call.role != "assistant" || result.role != "tool" {
        return None;
    }
    let [call_block] = call.content().as_slice() else {
        return None;
    };
    let [result_block] = result.content().as_slice() else {
        return None;
    };
    let BlockKind::ToolCall { id: call_id, .. } = call_block.kind() else {
        return None;
    };
    let BlockKind::ToolResult { id: result_id, .. } = result_block.kind() else {
        return None;
    };
    (call_id == result_id).then(|| render_tool_pair_as_part(call_block, result_block))
}

fn render_synthetic_todo_pair(call: &WireMessage, result: &WireMessage) -> Option<Value> {
    if !call.meta.synthetic
        || !result.meta.synthetic
        || call.role != "assistant"
        || result.role != "tool"
    {
        return None;
    }
    let BlockKind::ToolCall { id, name, .. } = call.content().first()?.kind().clone() else {
        return None;
    };
    let BlockKind::ToolResult {
        id: result_id,
        output,
        ..
    } = result.content().first()?.kind().clone()
    else {
        return None;
    };
    if id != result_id || !id.starts_with("synthetic_todo_") {
        return None;
    }
    let OutputKind::Json { value } = output.kind else {
        return None;
    };
    Some(json!({
        "type": "tool",
        "callID": id,
        "tool": name,
        "state": value,
        "syntheticTodoMarker": true,
    }))
}

fn synthetic_message_info(msg: &WireMessage, session_id: Option<&str>) -> Value {
    let mut info = json!({ "role": msg.role });
    if let Some(session_id) = session_id {
        set_value(
            &mut info,
            "sessionID",
            Value::String(session_id.to_string()),
        );
    } else {
        let id =
            msg.meta.harness_id.clone().unwrap_or_else(|| {
                format!("opencode-{}", stable_hash_prefix(&json!(msg.role), 12))
            });
        set_value(&mut info, "id", Value::String(id));
    }
    info
}

fn encode_new_message(msg: &WireMessage, session_id: Option<&str>) -> Value {
    let id = msg
        .meta
        .harness_id
        .clone()
        .unwrap_or_else(|| format!("opencode-{}", stable_hash_prefix(&json!(msg.role), 12)));
    let mut parts = Vec::new();
    let mut index = 0;
    while index < msg.content().len() {
        let block = &msg.content()[index];
        if let BlockKind::ToolCall { id, .. } = block.kind()
            && let Some(next) = msg.content().get(index + 1)
            && matches!(next.kind(), BlockKind::ToolResult { id: result_id, .. } if result_id == id)
        {
            parts.push(render_tool_pair_as_part(block, next));
            index += 2;
            continue;
        }
        parts.push(render_block_as_part(block));
        index += 1;
    }
    if msg.meta.synthetic && msg.role == "user" {
        for part in &mut parts {
            set_value(part, "synthetic", Value::Bool(true));
        }
    }
    let info = if msg.meta.synthetic && msg.role == "user" {
        let mut info = json!({ "role": msg.role });
        if let Some(session_id) = session_id {
            set_value(
                &mut info,
                "sessionID",
                Value::String(session_id.to_string()),
            );
        } else {
            set_value(&mut info, "id", Value::String(id));
        }
        info
    } else {
        let role = if msg.role == "tool" {
            // MessageV2 model conversion only visits tool parts inside assistant messages.
            "assistant"
        } else {
            msg.role.as_str()
        };
        json!({ "id": id, "role": role })
    };
    json!({ "info": info, "parts": parts })
}

fn render_block_as_part(block: &WireBlock) -> Value {
    match block.kind() {
        BlockKind::Text { text } => json!({ "type": "text", "text": text }),
        BlockKind::Reasoning { text, signature } => {
            let mut part = json!({ "type": "reasoning", "text": text });
            if let Some(signature) = signature {
                set_value(&mut part, "metadata", json!({ "signature": signature }));
            }
            part
        }
        BlockKind::RedactedReasoning { data } => {
            json!({ "type": "reasoning", "text": "", "metadata": { "redacted": data } })
        }
        BlockKind::ToolCall {
            id,
            name,
            input,
            provider_executed,
        } => {
            let mut part = json!({
                "type": "tool",
                "callID": id,
                "tool": name,
                "state": {
                    "status": "running",
                    "input": input,
                    "time": synthetic_tool_time(),
                },
            });
            if *provider_executed {
                set_nested_value(&mut part, "metadata", "providerExecuted", Value::Bool(true));
            }
            part
        }
        BlockKind::ToolResult {
            id,
            tool_name,
            output,
            ..
        } => {
            let mut part = json!({
                "type": "tool",
                "callID": id,
                "tool": tool_name,
                "state": {
                    "input": {},
                    "time": synthetic_tool_time(),
                }
            });
            apply_tool_output_to_part(&mut part, output);
            part
        }
        BlockKind::Media(media) => render_media_part(media),
        BlockKind::Opaque(opaque) => opaque.raw.clone(),
    }
}

fn render_tool_pair_as_part(call: &WireBlock, result: &WireBlock) -> Value {
    let mut part = render_block_as_part(call);
    let BlockKind::ToolResult { output, .. } = result.kind() else {
        update_part_from_block(&mut part, result);
        return part;
    };

    apply_tool_output_to_part(&mut part, output);
    set_nested_value(&mut part, "state", "time", synthetic_tool_time());
    part
}

fn synthetic_tool_time() -> Value {
    json!({
        "start": SYNTHETIC_TIMESTAMP,
        "end": SYNTHETIC_TIMESTAMP,
    })
}

fn render_media_part(media: &MediaBlock) -> Value {
    let mut part = json!({
        "type": "file",
        "mime": media.media_type,
    });
    if let Some(filename) = &media.filename {
        set_string(&mut part, "filename", filename);
    }
    if let Some(obj) = media.source.as_object() {
        match obj.get("type").and_then(Value::as_str) {
            Some("data_base64") => {
                if let Some(data) = obj.get("data").and_then(Value::as_str) {
                    set_string(
                        &mut part,
                        "url",
                        &format!("data:{};base64,{data}", media.media_type),
                    );
                }
            }
            Some("url") => {
                if let Some(url) = obj.get("url").and_then(Value::as_str) {
                    set_string(&mut part, "url", url);
                }
            }
            _ => {}
        }
    }
    part
}

fn output_status_text(output: &ToolOutput) -> (&'static str, String) {
    match &output.kind {
        OutputKind::Text { text } => ("completed", text.clone()),
        OutputKind::Json { value } => ("completed", value.to_string()),
        OutputKind::ErrorText { text } => ("error", text.clone()),
        OutputKind::ErrorJson { value } => ("error", value.to_string()),
        OutputKind::ExecutionDenied { reason } => (
            "error",
            reason
                .clone()
                .unwrap_or_else(|| "Execution denied".to_string()),
        ),
        OutputKind::Content { blocks } | OutputKind::ErrorContent { blocks } => {
            let text = blocks
                .iter()
                .filter_map(|block| match &block.kind {
                    ResultBlockKind::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let status = if matches!(&output.kind, OutputKind::ErrorContent { .. }) {
                "error"
            } else {
                "completed"
            };
            (status, text)
        }
    }
}

fn output_attachments(output: &ToolOutput) -> Vec<Value> {
    let blocks = match &output.kind {
        OutputKind::Content { blocks } | OutputKind::ErrorContent { blocks } => blocks,
        _ => return Vec::new(),
    };
    blocks
        .iter()
        .filter_map(|block| match &block.kind {
            ResultBlockKind::Text { .. } => None,
            ResultBlockKind::Media { media } => Some(render_tool_attachment(block, media)),
            ResultBlockKind::Opaque { opaque } => Some(opaque.raw.clone()),
        })
        .collect()
}

fn render_tool_attachment(block: &ResultBlock, media: &MediaBlock) -> Value {
    let Some(mut retained) = block
        .provider_extras
        .get(HARNESS)
        .and_then(|namespace| namespace.get("rawAttachment"))
        .cloned()
    else {
        return render_media_part(media);
    };
    if media_from_part(&retained) == *media {
        return retained;
    }
    let fresh = render_media_part(media);
    let Some(retained_object) = retained.as_object_mut() else {
        return fresh;
    };
    for key in [
        "type", "mime", "mimeType", "filename", "name", "url", "data",
    ] {
        retained_object.remove(key);
    }
    if let Some(fresh_object) = fresh.as_object() {
        retained_object.extend(fresh_object.clone());
    }
    retained
}

fn apply_tool_output_to_part(part: &mut Value, output: &ToolOutput) {
    let (status, text) = output_status_text(output);
    set_nested_value(part, "state", "status", Value::String(status.to_string()));
    let output_key = if status == "error" { "error" } else { "output" };
    let stale_key = if status == "error" { "output" } else { "error" };
    remove_nested_value(part, "state", stale_key);
    set_nested_value(part, "state", output_key, Value::String(text));

    let attachments = output_attachments(output);
    if attachments.is_empty() {
        remove_nested_value(part, "state", "attachments");
    } else {
        set_nested_value(part, "state", "attachments", Value::Array(attachments));
    }
}

fn opaque_block(kind: &str, raw: Value, arc: Option<Value>) -> WireBlock {
    super::json::opaque_block(HARNESS, kind, raw, arc)
}

fn redacted_reasoning_data(part: &Value) -> Option<String> {
    string_field(part, "data")
        .or_else(|| string_field(part, "redacted"))
        .or_else(|| {
            part.get("metadata")
                .and_then(|m| string_field(m, "redacted"))
        })
}

fn find_signature(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(signature) = map.get("signature").and_then(Value::as_str) {
                return Some(signature.to_string());
            }
            map.values().find_map(find_signature)
        }
        Value::Array(values) => values.iter().find_map(find_signature),
        _ => None,
    }
}

fn tool_name(part: &Value) -> String {
    string_field(part, "tool")
        .or_else(|| string_field(part, "toolName"))
        .or_else(|| string_field(part, "name"))
        .unwrap_or_else(|| "tool".to_string())
}

fn tool_status(part: &Value) -> Option<String> {
    part.get("state")
        .and_then(|state| string_field(state, "status"))
        .or_else(|| string_field(part, "status"))
}

fn is_synthetic_message(parts: &[Value]) -> bool {
    !parts.is_empty() && parts.iter().all(is_synthetic_part)
}

fn remove_nested_value(value: &mut Value, object_key: &str, key: &str) {
    if let Some(nested) = value.get_mut(object_key).and_then(Value::as_object_mut) {
        nested.remove(key);
    }
}

fn set_nested_value(value: &mut Value, object_key: &str, key: &str, next: Value) {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let entry = obj
        .entry(object_key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Some(nested) = entry.as_object_mut() {
        nested.insert(key.to_string(), next);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::wire::MediaKind;

    pub(crate) const PARITY_FIXTURE: &str = include_str!(
        "../../../../packages/opencode-plugin/src/hooks/context/__fixtures__/opencode-codec-parity.json"
    );

    pub(crate) fn native_window(case: &Value) -> Vec<Arc<Value>> {
        case["messages"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(Arc::new)
            .collect()
    }

    /// The corpus windows whose every message is an object carrying `info.id` and whose call
    /// ids are unique.
    const IDENTIFIED_WINDOWS: [&str; 7] = [
        "thinking",
        "cache_control",
        "summary_finish_and_error",
        "timestamps",
        "opaque_media_and_excluded_parts",
        "one_compaction",
        "multiple_compactions",
    ];

    /// The plugin encoder is the oracle: for every corpus window the daemon derives the CK the
    /// plugin encodes, equal as typed values and as canonical bytes.
    #[test]
    fn the_daemon_derives_the_window_the_opencode_plugin_encodes() {
        let fixture: Value = serde_json::from_str(PARITY_FIXTURE).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let derived = derive_opencode_window(&native_window(case)).unwrap();
            let plugin: Vec<IngressMessage> =
                serde_json::from_value(case["expected"].clone()).unwrap();
            assert_eq!(derived, plugin, "{name}");
            assert_eq!(
                serde_json::to_string(&derived).unwrap(),
                serde_json::to_string(&plugin).unwrap(),
                "{name}"
            );
        }
    }

    /// Compaction summary rows leave the window, the last compaction part is the boundary the
    /// sidecar decode extracts, and summary and finish rows that are not compaction summaries
    /// stay with their flags.
    #[test]
    fn the_compaction_pin_holds_for_one_and_several_compactions() {
        let fixture: Value = serde_json::from_str(PARITY_FIXTURE).unwrap();
        let window = |name: &str| {
            native_window(
                fixture["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|case| case["name"] == name)
                    .unwrap(),
            )
        };
        let case = |name: &str| derive_opencode_window(&window(name)).unwrap();
        let mids = |messages: &[IngressMessage]| {
            messages
                .iter()
                .map(|message| message.mid.clone())
                .collect::<Vec<_>>()
        };

        let one = case("one_compaction");
        assert_eq!(mids(&one), ["msg_k1", "msg_k2", "msg_k3", "msg_k5"]);
        let boundary = decode_opencode_shared(&window("one_compaction"))
            .boundary
            .unwrap();
        assert_eq!(
            (boundary.message_id.as_str(), boundary.part_index),
            ("msg_k3", Some(0))
        );
        assert!(one[2].ck.content().is_empty());

        let several = case("multiple_compactions");
        assert_eq!(mids(&several), ["msg_m1", "msg_m3", "msg_m4", "msg_m6"]);
        let boundary = decode_opencode_shared(&window("multiple_compactions"))
            .boundary
            .unwrap();
        assert_eq!(
            (boundary.message_id.as_str(), boundary.part_index),
            ("msg_m4", Some(1))
        );
        assert_eq!(boundary.raw, json!({ "type": "compaction", "auto": false }));

        let flagged = case("summary_finish_and_error");
        let meta = |mid: &str| {
            &flagged
                .iter()
                .find(|message| message.mid == mid)
                .unwrap()
                .ck
                .meta
        };
        assert!(meta("msg_s3").summary);
        assert_eq!(meta("msg_s3").finish.as_deref(), Some("length"));
        assert!(meta("msg_s6").summary && meta("msg_s6").finish.is_none());
        assert_eq!(meta("msg_s2").finish.as_deref(), Some("tool-calls"));
        assert!(meta("msg_s4").errored && !meta("msg_s5").errored);
        // Only `summary: true` with `finish: "stop"` marks a compaction summary.
        for mid in ["msg_s7", "msg_s8", "msg_s9", "msg_s10"] {
            assert_eq!(meta(mid).finish.as_deref(), Some("stop"), "{mid}");
        }
    }

    /// The sidecar decode of each corpus window of identified messages encodes back to the
    /// window, so the shared decode loop keeps native attachment exact for those shapes. The
    /// encoder names an id-less message and holds every served call id unique, so the windows
    /// built on those inputs encode to the named, deduplicated form instead.
    #[test]
    fn every_identified_parity_window_round_trips_through_the_sidecar() {
        let fixture: Value = serde_json::from_str(PARITY_FIXTURE).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            if !IDENTIFIED_WINDOWS.contains(&name) {
                continue;
            }
            let window = native_window(case);
            let decoded = decode_opencode_shared(&window);
            let messages: Vec<_> = decoded.messages.iter().map(|m| m.ck.clone()).collect();
            let encoded = encode_opencode_with_session(&messages, &decoded.sidecar, None, None);
            let raw: Vec<Value> = window
                .iter()
                .map(|message| message.as_ref().clone())
                .collect();
            assert_eq!(encoded, raw, "{name}");
        }
    }

    fn fresh_tool_transform_fixture() -> Vec<WireMessage> {
        let paired_id = "folded-call";
        let paired_call = WireBlock::bare(BlockKind::ToolCall {
            id: paired_id.to_string(),
            name: "read".to_string(),
            input: json!({ "path": "covered.txt" }),
            provider_executed: false,
        });
        let paired_result = WireBlock::bare(BlockKind::ToolResult {
            id: paired_id.to_string(),
            tool_name: "read".to_string(),
            output: ToolOutput::bare(OutputKind::Text {
                text: "folded output".to_string(),
            }),
            provider_executed: false,
        });
        let standalone_call = WireBlock::bare(BlockKind::ToolCall {
            id: "standalone-call".to_string(),
            name: "write".to_string(),
            input: json!({ "path": "new.txt", "content": "hello" }),
            provider_executed: false,
        });
        let standalone_result = WireBlock::bare(BlockKind::ToolResult {
            id: "standalone-result".to_string(),
            tool_name: "write".to_string(),
            output: ToolOutput::bare(OutputKind::ErrorContent {
                blocks: vec![
                    ResultBlock {
                        kind: ResultBlockKind::Text {
                            text: "write denied".to_string(),
                        },
                        provider_extras: ProviderExtras::new(),
                    },
                    ResultBlock {
                        kind: ResultBlockKind::Media {
                            media: MediaBlock {
                                kind: MediaKind::Image,
                                media_type: "image/png".to_string(),
                                filename: Some("error.png".to_string()),
                                source: json!({
                                    "type": "data_base64",
                                    "data": "aW1hZ2U="
                                }),
                            },
                        },
                        provider_extras: ProviderExtras::new(),
                    },
                ],
            }),
            provider_executed: false,
        });

        vec![
            WireMessage::from_parts(
                "assistant",
                vec![paired_call, paired_result],
                None,
                ProviderExtras::new(),
                HarnessMeta {
                    harness_id: Some("folded-tool-arc".to_string()),
                    ..Default::default()
                },
            ),
            WireMessage::from_parts(
                "assistant",
                vec![standalone_call],
                None,
                ProviderExtras::new(),
                HarnessMeta {
                    harness_id: Some("standalone-call-message".to_string()),
                    ..Default::default()
                },
            ),
            WireMessage::from_parts(
                "tool",
                vec![standalone_result],
                None,
                ProviderExtras::new(),
                HarnessMeta {
                    harness_id: Some("standalone-result-message".to_string()),
                    ..Default::default()
                },
            ),
        ]
    }

    #[test]
    fn every_fresh_tool_part_from_folded_transform_fixture_has_complete_state() {
        let encoded = encode_opencode(
            &fresh_tool_transform_fixture(),
            &DecodeSidecar::new(HARNESS),
            None,
        );
        let mut tool_part_count = 0;

        for message in &encoded {
            for part in message["parts"].as_array().unwrap() {
                if part["type"] != "tool" {
                    continue;
                }
                tool_part_count += 1;
                assert!(
                    part.get("callID").and_then(Value::as_str).is_some(),
                    "tool part has no callID: {part}"
                );
                assert!(
                    part.get("tool").and_then(Value::as_str).is_some(),
                    "tool part has no tool name: {part}"
                );
                let state = part["state"].as_object().unwrap();
                assert!(
                    state.get("status").is_some(),
                    "tool part has no status: {part}"
                );
                assert!(
                    state
                        .get("time")
                        .and_then(Value::as_object)
                        .and_then(|time| time.get("start"))
                        .is_some(),
                    "tool part has no state.time.start: {part}"
                );
                assert!(
                    state
                        .get("time")
                        .and_then(Value::as_object)
                        .and_then(|time| time.get("end"))
                        .is_some(),
                    "tool part has no state.time.end: {part}"
                );
            }
        }

        assert_eq!(
            tool_part_count, 3,
            "fixture did not exercise all fresh tool arms"
        );
        assert_eq!(encoded[0]["parts"][0]["state"]["status"], "completed");
        assert_eq!(
            encoded[0]["parts"][0]["state"]["input"],
            json!({ "path": "covered.txt" })
        );
        assert_eq!(encoded[1]["parts"][0]["state"]["status"], "running");
        assert_eq!(encoded[2]["info"]["role"], "assistant");
        assert_eq!(encoded[2]["parts"][0]["callID"], "standalone-result");
        assert_eq!(encoded[2]["parts"][0]["tool"], "write");
        assert_eq!(encoded[2]["parts"][0]["state"]["status"], "error");
        assert_eq!(encoded[2]["parts"][0]["state"]["input"], json!({}));
        assert_eq!(encoded[2]["parts"][0]["state"]["error"], "write denied");
        assert_eq!(
            encoded[2]["parts"][0]["state"]["attachments"],
            json!([{
                "type": "file",
                "mime": "image/png",
                "filename": "error.png",
                "url": "data:image/png;base64,aW1hZ2U="
            }])
        );
    }

    #[test]
    fn fresh_tool_parts_are_byte_identical_across_consecutive_defer_passes() {
        let fixture = fresh_tool_transform_fixture();
        let sidecar = DecodeSidecar::new(HARNESS);
        let first = encode_opencode(&fixture, &sidecar, None);
        let second = encode_opencode(&fixture, &sidecar, None);
        let first_tool_parts = first
            .iter()
            .flat_map(|message| message["parts"].as_array().unwrap())
            .filter(|part| part["type"] == "tool")
            .cloned()
            .collect::<Vec<_>>();
        let second_tool_parts = second
            .iter()
            .flat_map(|message| message["parts"].as_array().unwrap())
            .filter(|part| part["type"] == "tool")
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(
            serde_json::to_vec(&first_tool_parts).unwrap(),
            serde_json::to_vec(&second_tool_parts).unwrap()
        );
        assert!(first_tool_parts.iter().all(|part| {
            part["state"]["time"]["start"] == SYNTHETIC_TIMESTAMP
                && part["state"]["time"]["end"] == SYNTHETIC_TIMESTAMP
        }));
    }

    #[test]
    fn adjacent_equal_kind_deletion_retains_the_surviving_native_part() {
        let raw = vec![json!({
            "info": { "id": "adjacent-text", "role": "assistant" },
            "parts": [
                { "type": "text", "text": "A", "vendorPart": "A" },
                {
                    "type": "text",
                    "text": "DELETE",
                    "time": { "start": 2 },
                    "vendorPart": "delete"
                },
                {
                    "type": "text",
                    "text": "SURVIVE",
                    "time": { "start": 3 },
                    "vendorPart": "survive"
                }
            ]
        })];
        let decoded = decode_opencode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(1);

        let encoded = encode_opencode(&[message], &decoded.sidecar, None);
        assert_eq!(
            encoded[0]["parts"],
            json!([
                { "type": "text", "text": "A", "vendorPart": "A" },
                {
                    "type": "text",
                    "text": "SURVIVE",
                    "time": { "start": 3 },
                    "vendorPart": "survive"
                }
            ])
        );
    }

    /// An edited block beside a deleted sibling has no native part its content or position
    /// names, so it renders as a fresh part; the unedited neighbor keeps its native extras.
    #[test]
    fn an_edited_survivor_beside_a_deletion_renders_as_a_fresh_part() {
        let raw = vec![json!({
            "info": { "id": "mutated-adjacent-text", "role": "assistant" },
            "parts": [
                { "type": "text", "text": "A", "vendorPart": "A" },
                { "type": "text", "text": "DELETE", "vendorPart": "delete" },
                { "type": "text", "text": "SURVIVE", "vendorPart": "survive" }
            ]
        })];
        let decoded = decode_opencode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(1);
        let survivor = &mut message.content_mut()[1];
        *survivor.kind_mut() = BlockKind::Text {
            text: "§3§ SURVIVE".to_string(),
        };

        let encoded = encode_opencode(&[message], &decoded.sidecar, None);
        assert_eq!(
            encoded[0]["parts"],
            json!([
                { "type": "text", "text": "A", "vendorPart": "A" },
                { "type": "text", "text": "§3§ SURVIVE" }
            ])
        );
    }

    #[test]
    fn tool_block_metas_carry_the_call_id_and_a_repeated_call_adds_only_its_result() {
        let tool = |part_id: &str, output: &str| {
            json!({
                "id": part_id,
                "type": "tool",
                "callID": "call_a",
                "tool": "read",
                "state": { "status": "completed", "input": {}, "output": output }
            })
        };
        let raw = vec![json!({
            "info": { "id": "msg_tools", "role": "assistant" },
            "parts": [tool("prt_1", "one"), tool("prt_2", "two")]
        })];
        let decoded = decode_opencode(&raw);
        let metas = decoded.sidecar.messages["msg_tools"]
            .blocks
            .iter()
            .map(|meta| {
                (
                    meta.kind.as_str(),
                    meta.native_id.as_deref(),
                    meta.native_index,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            metas,
            [
                ("tool_call", Some("call_a"), Some(0)),
                ("tool_result", Some("call_a"), Some(0)),
                ("tool_result", Some("call_a"), Some(1)),
            ]
        );
    }

    #[test]
    fn adjacent_idless_tools_match_their_synthesized_identity() {
        let raw = vec![json!({
            "info": { "id": "idless-tools", "role": "assistant" },
            "parts": [
                {
                    "type": "tool",
                    "tool": "read",
                    "vendorPart": "delete",
                    "state": { "status": "running", "input": { "path": "delete" } }
                },
                {
                    "type": "tool",
                    "tool": "read",
                    "vendorPart": "survive",
                    "state": { "status": "running", "input": { "path": "survive" } }
                }
            ]
        })];
        let decoded = decode_opencode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        let first_id = match message.content()[0].kind() {
            BlockKind::ToolCall { id, .. } => id.clone(),
            _ => panic!("expected first tool call"),
        };
        let second_id = match message.content()[1].kind() {
            BlockKind::ToolCall { id, .. } => id.clone(),
            _ => panic!("expected second tool call"),
        };
        assert_ne!(first_id, second_id);
        assert_eq!(
            decoded.sidecar.messages["idless-tools"].blocks[1]
                .native_id
                .as_deref(),
            Some(second_id.as_str())
        );
        message.content_mut().remove(0);

        let encoded = encode_opencode(&[message], &decoded.sidecar, None);
        assert_eq!(encoded[0]["parts"], json!([raw[0]["parts"][1].clone()]));
    }

    #[test]
    fn completed_and_error_attachments_round_trip_with_native_polarity() {
        let raw = vec![
            json!({
                "info": { "id": "completed-attachment", "role": "assistant" },
                "parts": [{
                    "type": "tool",
                    "callID": "call-ok",
                    "tool": "read",
                    "state": {
                        "status": "completed",
                        "input": {},
                        "output": "done",
                        "attachments": [{
                            "mime": "image/png",
                            "url": "data:image/png;base64,b2s=",
                            "vendor": "keep-ok"
                        }],
                        "time": { "start": 1, "end": 2 }
                    }
                }]
            }),
            json!({
                "info": { "id": "error-attachment", "role": "assistant" },
                "parts": [{
                    "type": "tool",
                    "callID": "call-err",
                    "tool": "read",
                    "state": {
                        "status": "error",
                        "input": {},
                        "error": "failed",
                        "attachments": [
                            {
                                "mime": "image/png",
                                "url": "data:image/png;base64,ZXJy",
                                "vendor": "keep-error"
                            },
                            { "type": "vendor-attachment", "payload": { "keep": true } }
                        ],
                        "time": { "start": 3, "end": 4 }
                    }
                }]
            }),
        ];
        let decoded = decode_opencode(&raw);
        let completed_output = match decoded.messages[0].ck.content()[1].kind() {
            BlockKind::ToolResult { output, .. } => output,
            _ => panic!("expected completed tool result"),
        };
        let error_output = match decoded.messages[1].ck.content()[1].kind() {
            BlockKind::ToolResult { output, .. } => output,
            _ => panic!("expected error tool result"),
        };
        assert!(matches!(completed_output.kind, OutputKind::Content { .. }));
        assert!(matches!(error_output.kind, OutputKind::ErrorContent { .. }));
        assert_eq!(
            encode_opencode(
                &decoded
                    .messages
                    .iter()
                    .map(|message| message.ck.clone())
                    .collect::<Vec<_>>(),
                &decoded.sidecar,
                None,
            ),
            raw
        );

        let mut error_message = decoded.messages[1].ck.clone();
        let result = &mut error_message.content_mut()[1];
        let BlockKind::ToolResult { output, .. } = result.kind_mut() else {
            panic!("expected error tool result");
        };
        let OutputKind::ErrorContent { blocks } = &mut output.kind else {
            panic!("expected ErrorContent");
        };
        let ResultBlockKind::Text { text } = &mut blocks[0].kind else {
            panic!("expected leading text block");
        };
        *text = "tagged failure".to_string();
        let encoded = encode_opencode(&[error_message], &decoded.sidecar, None);
        assert_eq!(encoded[0]["parts"][0]["state"]["status"], "error");
        assert_eq!(encoded[0]["parts"][0]["state"]["error"], "tagged failure");
        assert_eq!(
            encoded[0]["parts"][0]["state"]["attachments"],
            raw[1]["parts"][0]["state"]["attachments"]
        );
        assert_eq!(
            encoded[0]["parts"][0]["state"]["time"],
            raw[1]["parts"][0]["state"]["time"]
        );
    }

    #[test]
    fn adjacent_fresh_call_and_result_coalesce_for_message_v2_conversion() {
        let call = WireMessage::from_parts(
            "assistant",
            vec![WireBlock::bare(BlockKind::ToolCall {
                id: "call-7".to_string(),
                name: "inspect".to_string(),
                input: json!({ "path": "artifact.bin" }),
                provider_executed: false,
            })],
            None,
            ProviderExtras::new(),
            HarnessMeta::default(),
        );
        let result = WireMessage::from_parts(
            "tool",
            vec![WireBlock::bare(BlockKind::ToolResult {
                id: "call-7".to_string(),
                tool_name: "inspect".to_string(),
                output: ToolOutput::bare(OutputKind::Text {
                    text: "done".to_string(),
                }),
                provider_executed: false,
            })],
            None,
            ProviderExtras::new(),
            HarnessMeta::default(),
        );

        let encoded = encode_opencode(&[call, result], &DecodeSidecar::new(HARNESS), None);
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0]["info"]["role"], "assistant");
        assert_eq!(encoded[0]["parts"][0]["callID"], "call-7");
        assert_eq!(encoded[0]["parts"][0]["tool"], "inspect");
        assert_eq!(
            encoded[0]["parts"][0]["state"]["input"],
            json!({ "path": "artifact.bin" })
        );
        assert_eq!(encoded[0]["parts"][0]["state"]["status"], "completed");
        assert_eq!(encoded[0]["parts"][0]["state"]["output"], "done");
    }

    #[test]
    fn overlay_edited_ingress_parts_keep_their_native_slots() {
        let raw = vec![json!({
            "info": { "id": "msg_tagged", "role": "assistant" },
            "parts": [
                { "id": "prt_start", "type": "step-start" },
                { "id": "prt_text", "type": "text", "text": "Queue is x.", "time": { "start": 1, "end": 2 } },
                { "id": "prt_tool", "type": "tool", "callID": "call_1", "tool": "bash",
                  "state": { "status": "completed", "input": { "command": "ls" }, "output": "out",
                             "time": { "start": 3, "end": 4 } } },
                { "id": "prt_finish", "type": "step-finish", "reason": "tool-calls" }
            ]
        })];
        let decoded = decode_opencode(&raw);
        // Ingress blocks arrive from the plugin's wire build, which carries no codec stamp.
        let mut message = decoded.messages[0].ck.clone();
        for block in message.content_mut() {
            block.provider_extras.clear();
            match block.kind_mut() {
                BlockKind::Text { text } => *text = format!("§3§ {text}"),
                BlockKind::ToolResult { output, .. } => {
                    output.kind = OutputKind::Text {
                        text: "§4§ out".to_string(),
                    };
                }
                _ => {}
            }
        }

        let served = encode_opencode(&[message], &decoded.sidecar, None);
        let parts = served[0]["parts"].as_array().unwrap();
        let ids = parts
            .iter()
            .map(|part| part["id"].as_str().unwrap_or("<fresh>"))
            .collect::<Vec<_>>();
        assert_eq!(ids, ["prt_start", "prt_text", "prt_tool", "prt_finish"]);
        assert_eq!(parts[1]["text"], "§3§ Queue is x.");
        assert_eq!(parts[1]["time"], raw[0]["parts"][1]["time"]);
        assert_eq!(parts[2]["state"]["output"], "§4§ out");
    }

    #[test]
    fn value_equal_envelope_keeps_native_float_zero_signs() {
        let raw = vec![json!({
            "info": { "id": "msg_zero", "role": "assistant" },
            "parts": [{
                "id": "prt_tool", "type": "tool", "callID": "call_zero", "tool": "probe",
                "state": { "status": "running", "input": { "offset": -0.0 } }
            }]
        })];
        let decoded = decode_opencode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        // The served envelope equals the native one as a JSON value, so the host receives its
        // own message back, signs included; an envelope that differs anywhere else renders the
        // served numbers.
        for block in message.content_mut() {
            if let BlockKind::ToolCall { input, .. } = block.kind_mut() {
                *input = json!({ "offset": 0.0 });
            }
        }

        let served = encode_opencode(&[message], &decoded.sidecar, None);
        assert_eq!(
            serde_json::to_string(&served[0]).unwrap(),
            serde_json::to_string(&raw[0]).unwrap()
        );
    }

    /// The unchanged-envelope shortcut returns the retained envelope only where the full render
    /// returns it: over the parity corpus, the golden windows, and envelopes the shortcut must
    /// decline, under unchanged, re-roled, and dropped-block encodes with and without
    /// compaction parts, every message the shortcut accepts renders to its retained `Arc`.
    #[test]
    fn the_unchanged_envelope_shortcut_agrees_with_the_full_render() {
        let parity: Value = serde_json::from_str(PARITY_FIXTURE).unwrap();
        let golden: Value =
            serde_json::from_str(include_str!("../../testdata/codec/opencode-golden.json"))
                .unwrap();
        let mut windows: Vec<Vec<Arc<Value>>> = parity["cases"]
            .as_array()
            .unwrap()
            .iter()
            .chain(golden["cases"].as_array().unwrap())
            .map(native_window)
            .collect();
        windows.push(
            [
                json!({ "info": { "id": "no-parts", "role": "user" } }),
                json!({ "info": { "id": "text-parts", "role": "user" }, "parts": "text" }),
                json!({ "id": "outer", "info": { "role": "user" },
                        "parts": [{ "type": "text", "text": "x" }] }),
                json!({ "info": { "id": "compacted", "role": "user" },
                        "parts": [{ "type": "compaction" }, { "type": "text", "text": "y" }] }),
            ]
            .into_iter()
            .map(Arc::new)
            .collect(),
        );
        let (mut accepted, mut declined) = (0, 0);
        for window in &windows {
            let decoded = decode_opencode_shared(window);
            for message in &decoded.messages {
                let meta = decoded.sidecar.message_by_mid(&message.mid).unwrap();
                let mut variants = vec![message.ck.clone()];
                let mut re_roled = message.ck.clone();
                re_roled.role = if re_roled.role == "user" {
                    "assistant"
                } else {
                    "user"
                }
                .into();
                variants.push(re_roled);
                if !message.ck.content().is_empty() {
                    let mut dropped = message.ck.clone();
                    dropped.content_mut().remove(0);
                    variants.push(dropped);
                }
                for msg in &variants {
                    for preserve_compaction in [true, false] {
                        let matched = align_with_meta(msg, meta);
                        if !envelope_is_unchanged(msg, meta, &matched, preserve_compaction) {
                            declined += 1;
                            continue;
                        }
                        accepted += 1;
                        let full = render_with_meta(msg, meta, &matched, preserve_compaction);
                        assert!(Arc::ptr_eq(&full, &meta.raw), "{}", message.mid);
                    }
                }
            }
        }
        assert!(accepted > 0 && declined > 0, "{accepted} {declined}");

        let no_parts = decode_opencode(&[json!({ "info": { "id": "no-parts", "role": "user" } })]);
        let encoded = encode_opencode_shared(
            &[no_parts.messages[0].ck.clone()],
            &no_parts.sidecar,
            None,
            &[],
        );
        assert_eq!(
            *encoded[0],
            json!({ "info": { "id": "no-parts", "role": "user" }, "parts": [] })
        );
    }

    /// A caller-supplied codec namespace carries no origin: over the identified parity windows,
    /// served blocks that hold one, unchanged or edited, encode to the bytes the same blocks
    /// encode to without it.
    #[test]
    fn a_caller_supplied_codec_namespace_changes_no_encoded_byte() {
        let parity: Value = serde_json::from_str(PARITY_FIXTURE).unwrap();
        for case in parity["cases"].as_array().unwrap() {
            if !IDENTIFIED_WINDOWS.contains(&case["name"].as_str().unwrap()) {
                continue;
            }
            let window = native_window(case);
            let decoded = decode_opencode_shared(&window);
            let mut bare: Vec<WireMessage> = decoded
                .messages
                .iter()
                .map(|message| message.ck.clone())
                .collect();
            if let Some(BlockKind::Text { text }) = bare
                .iter_mut()
                .flat_map(|message| message.content_mut().iter_mut())
                .map(WireBlock::kind_mut)
                .find(|kind| matches!(kind, BlockKind::Text { .. }))
            {
                text.insert_str(0, "§1§ ");
            }
            let mut forged = bare.clone();
            for (index, block) in forged
                .iter_mut()
                .flat_map(|message| message.content_mut().iter_mut())
                .enumerate()
            {
                let namespace = block
                    .provider_extras
                    .entry(crate::codec::sidecar::BLOCK_IDENTITY_NAMESPACE.to_string())
                    .or_default();
                namespace.insert("blockIndex".into(), Value::from(0));
                namespace.insert("nativeIndex".into(), Value::from(index));
                namespace.insert("decodedFingerprint".into(), Value::from("forged"));
            }
            let encode = |messages: &[WireMessage]| {
                serde_json::to_string(&encode_opencode_shared(
                    messages,
                    &decoded.sidecar,
                    None,
                    &[],
                ))
                .unwrap()
            };
            assert_eq!(encode(&forged), encode(&bare), "{}", case["name"]);
        }
    }

    #[test]
    fn tool_mutation_preserves_native_time_verbatim() {
        let raw = vec![json!({
            "info": { "id": "native-tool", "role": "assistant" },
            "parts": [{
                "type": "tool",
                "callID": "native-call",
                "tool": "bash",
                "state": {
                    "status": "completed",
                    "input": { "command": "printf native" },
                    "output": "native output",
                    "time": { "start": 12345, "end": 67890 }
                }
            }]
        })];
        let decoded = decode_opencode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        let result = message
            .content_mut()
            .iter_mut()
            .find(|block| matches!(block.kind(), BlockKind::ToolResult { .. }))
            .unwrap();
        if let BlockKind::ToolResult { output, .. } = result.kind_mut() {
            output.kind = OutputKind::Text {
                text: "§1§ tagged output".to_string(),
            };
        }

        let served = encode_opencode(&[message], &decoded.sidecar, None);
        assert_eq!(
            served[0]["parts"][0]["state"]["time"],
            raw[0]["parts"][0]["state"]["time"]
        );
        assert_eq!(
            served[0]["parts"][0]["state"]["output"],
            "§1§ tagged output"
        );
    }

    #[test]
    fn empty_text_stays_text_and_ignored_text_stays_opaque() {
        let raw = vec![json!({
            "info": { "id": "msg_1", "role": "user" },
            "parts": [
                { "type": "text", "text": "", "time": { "start": 1 } },
                { "type": "text", "text": "hidden", "ignored": true }
            ]
        })];
        let decoded = decode_opencode(&raw);
        // The plugin encodes an ignored text part as an opaque `text` block, so the CK keeps it.
        let content = decoded.messages[0].ck.content();
        assert_eq!(content.len(), 2);
        assert!(matches!(content[0].kind(), BlockKind::Text { text } if text.is_empty()));
        assert!(matches!(
            content[1].kind(),
            BlockKind::Opaque(opaque) if opaque.kind == "text" && opaque.raw == raw[0]["parts"][1]
        ));
        assert_eq!(
            encode_opencode(&[decoded.messages[0].ck.clone()], &decoded.sidecar, None),
            raw
        );
    }

    #[test]
    fn an_edited_text_block_never_lands_on_an_ignored_part() {
        let raw = vec![json!({
            "info": { "id": "msg_1", "role": "user" },
            "parts": [
                { "type": "text", "text": "hidden", "ignored": true },
                { "type": "text", "text": "shown" }
            ]
        })];
        let decoded = decode_opencode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(0);
        *message.content_mut()[0].kind_mut() = BlockKind::Text {
            text: "edited".to_string(),
        };
        let encoded = encode_opencode(&[message], &decoded.sidecar, None);
        let parts = encoded[0]["parts"].as_array().unwrap();
        assert!(
            parts
                .iter()
                .all(|part| part["ignored"] != true || part["text"] == "hidden"),
            "{parts:?}"
        );
        assert!(
            parts
                .iter()
                .any(|part| part["text"] == "edited" && part["ignored"] != true)
        );
    }

    #[test]
    fn synthetic_todo_marker_survives_collapsed_pair_decode() {
        let raw = vec![json!({
            "info": { "id": "msg_todo", "role": "assistant" },
            "parts": [{
                "type": "tool",
                "tool": "todowrite",
                "callID": "synthetic_todo_deadbeefdeadbeef",
                "syntheticTodoMarker": true,
                "state": {
                    "status": "completed",
                    "input": { "todos": [] },
                    "output": "[]"
                }
            }]
        })];

        let decoded = decode_opencode(&raw);

        assert!(decoded.messages[0].ck.meta.synthetic);
        assert_eq!(
            encode_opencode(&[decoded.messages[0].ck.clone()], &decoded.sidecar, None),
            raw
        );
    }

    #[test]
    fn compaction_is_extracted_as_boundary_not_content() {
        let raw = vec![json!({
            "info": { "id": "msg_boundary", "role": "user" },
            "parts": [
                { "type": "text", "text": "before" },
                { "type": "compaction", "auto": true }
            ]
        })];
        let decoded = decode_opencode(&raw);
        assert_eq!(decoded.messages[0].ck.content().len(), 1);
        assert_eq!(
            decoded.boundary.as_ref().unwrap().message_id,
            "msg_boundary"
        );
        assert_eq!(decoded.boundary.as_ref().unwrap().part_index, Some(1));
        let encoded = encode_opencode(&[decoded.messages[0].ck.clone()], &decoded.sidecar, None);
        let encoded_parts = encoded[0].get("parts").and_then(Value::as_array).unwrap();
        assert!(
            encoded_parts
                .iter()
                .all(|part| part.get("type").and_then(Value::as_str) != Some("compaction"))
        );
    }

    #[test]
    fn resolved_reasoning_exemption_and_completed_sibling_mutation_are_distinct() {
        let raw = vec![
            json!({
                "info": { "id": "msg_old", "role": "assistant" },
                "parts": [
                    { "type": "reasoning", "text": "old thinking", "metadata": { "signature": "old-sig" } },
                    { "type": "text", "text": "old answer" }
                ]
            }),
            json!({
                "info": { "id": "msg_latest", "role": "assistant", "providerField": "keep" },
                "parts": [
                    { "type": "step-start", "step": 7 },
                    { "type": "reasoning", "text": "latest thinking", "metadata": { "signature": "latest-sig" }, "providerField": "keep" },
                    { "type": "text", "text": "latest answer" },
                    { "type": "reasoning", "text": "latest second thinking", "metadata": { "signature": "latest-sig-2" } },
                    { "type": "step-finish", "reason": "stop" }
                ]
            }),
        ];
        let decoded = decode_opencode(&raw);
        let mut output = decoded
            .messages
            .iter()
            .map(|message| message.ck.clone())
            .collect::<Vec<_>>();

        let untouched = encode_opencode_with_session(
            &output,
            &decoded.sidecar,
            Some("ses_live"),
            Some("msg_latest"),
        );
        assert_eq!(
            untouched[1], raw[1],
            "the transform-resolved live exemption replays untouched ingress exactly"
        );

        let latest_text = output[1]
            .content_mut()
            .iter_mut()
            .find(|block| matches!(block.kind(), BlockKind::Text { .. }))
            .unwrap();
        *latest_text.kind_mut() = BlockKind::Text {
            text: "§7§ mutated latest answer".to_string(),
        };
        let served =
            encode_opencode_with_session(&output, &decoded.sidecar, Some("ses_live"), None);
        assert_eq!(served[1]["parts"][1], raw[1]["parts"][1]);
        assert_eq!(served[1]["parts"][3], raw[1]["parts"][3]);
        assert_eq!(served[1]["parts"][2]["text"], "§7§ mutated latest answer");
        assert_eq!(served[1]["info"]["providerField"], "keep");
    }

    #[test]
    fn lineage_anchor_and_live_assistant_exemptions_replay_native_envelopes_exactly() {
        let raw = vec![
            json!({
                "info": { "id": "summary", "role": "user", "providerField": "anchor-info" },
                "parts": [
                    { "type": "text", "text": "volatile date", "providerField": "date-part" },
                    { "type": "text", "text": "continuation summary", "providerField": "anchor-part" }
                ]
            }),
            json!({
                "info": { "id": "latest", "role": "assistant", "providerField": "live-info" },
                "parts": [
                    { "type": "reasoning", "text": "signed", "metadata": { "signature": "sig" } },
                    { "type": "text", "text": "answer", "providerField": "live-part" }
                ]
            }),
        ];
        let decoded = decode_opencode(&raw);
        let mut output = decoded
            .messages
            .iter()
            .map(|message| message.ck.clone())
            .collect::<Vec<_>>();
        *output[0].content_mut()[1].kind_mut() = BlockKind::Text {
            text: "overlay must not escape".to_string(),
        };
        *output[1].content_mut()[1].kind_mut() = BlockKind::Text {
            text: "strip must not escape".to_string(),
        };

        let served = encode_opencode_with_session_exemptions(
            &output,
            &decoded.sidecar,
            Some("ses_live"),
            &["summary", "latest"],
        );
        assert_eq!(served, raw);
    }

    #[test]
    fn hard_epoch_fold_with_head_todo_coalesces_unmatched_reduced_tool_arc() {
        let raw = vec![json!({
            "info": { "id": "first-tail-assistant", "role": "assistant" },
            "parts": [
                { "type": "step-start" },
                {
                    "type": "reasoning",
                    "text": "signed transition reasoning",
                    "metadata": { "signature": "transition-signature" }
                },
                {
                    "type": "tool",
                    "tool": "aft_zoom",
                    "callID": "call_uRXFDXYYs6UiMIkmAVDWZDzX",
                    "state": {
                        "status": "completed",
                        "input": { "path": "src/lib.rs", "symbols": ["target"] },
                        "output": "historical output"
                    }
                },
                { "type": "step-finish", "reason": "tool-calls" }
            ]
        })];
        let decoded = decode_opencode(&raw);
        assert!(matches!(
            decoded.messages[0].ck.content()[2].kind(),
            BlockKind::ToolCall { id, .. } if id == "call_uRXFDXYYs6UiMIkmAVDWZDzX"
        ));
        assert!(matches!(
            decoded.messages[0].ck.content()[3].kind(),
            BlockKind::ToolResult { id, .. } if id == "call_uRXFDXYYs6UiMIkmAVDWZDzX"
        ));
        let mut reduced_tail = decoded.messages[0].ck.clone();
        *reduced_tail.content_mut() = reduced_tail
            .content()
            .iter()
            .map(|block| {
                let reduced = match block.kind() {
                    BlockKind::ToolCall { .. } => {
                        crate::wire::reduced_block(block, "reduced call skeleton", None)
                    }
                    BlockKind::ToolResult { .. } => {
                        crate::wire::reduced_block(block, "[dropped]", None)
                    }
                    _ => block.clone(),
                };
                WireBlock::bare(reduced.kind().clone())
            })
            .collect();

        let active_todo = crate::injection::build_synthetic_todo_pair(
            r#"[{"content":"Inspect contributor issue","status":"in_progress","priority":"high"}]"#,
        )
        .unwrap();
        let served = vec![
            WireMessage::synthetic_user_text("m0"),
            WireMessage::synthetic_user_text("m1"),
            active_todo.assistant_msg,
            active_todo.tool_msg,
            reduced_tail,
        ];

        // A hard request after an epoch change must remain valid without a transition marker.
        let encoded = encode_opencode_with_session_exemptions(
            &served,
            &decoded.sidecar,
            Some("astro-epoch-change"),
            &[],
        );
        let tool_ids = encoded
            .iter()
            .flat_map(|message| message["parts"].as_array().into_iter().flatten())
            .filter(|part| part["type"] == "tool")
            .filter_map(|part| part["callID"].as_str())
            .collect::<Vec<_>>();
        let unique_ids = tool_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(
            tool_ids.len(),
            unique_ids.len(),
            "a reduced call/result shell must serialize as one native tool part: {tool_ids:?}"
        );
        assert_eq!(
            tool_ids
                .iter()
                .filter(|id| **id == "call_uRXFDXYYs6UiMIkmAVDWZDzX")
                .count(),
            1
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn whole_array_tool_use_guard_rejects_both_id_collision_directions() {
        let valid = vec![
            json!({
                "info": { "id": "first", "role": "assistant" },
                "parts": [{
                    "type": "tool",
                    "tool": "read",
                    "callID": "call-first",
                    "state": { "status": "pending", "input": {} }
                }]
            }),
            json!({
                "info": { "id": "second", "role": "assistant" },
                "parts": [{
                    "type": "tool",
                    "tool": "read",
                    "callID": "call-second",
                    "state": { "status": "pending", "input": {} }
                }]
            }),
        ];
        assert_unique_tool_use_ids(&valid);

        for (source, target) in [(0, 1), (1, 0)] {
            let mut collided = valid.clone();
            collided[target]["parts"][0]["callID"] = collided[source]["parts"][0]["callID"].clone();
            assert!(
                std::panic::catch_unwind(|| assert_unique_tool_use_ids(&collided)).is_err(),
                "copying the id from message {source} to message {target} must trip the guard"
            );
        }
    }

    #[test]
    fn text_before_latest_reasoning_uses_typed_wire_projection() {
        let raw = vec![json!({
            "info": { "id": "msg_text_first", "role": "assistant" },
            "parts": [
                { "type": "step-start" },
                { "type": "text", "text": "answer" },
                { "type": "reasoning", "text": "signed thinking", "metadata": { "signature": "sig" } }
            ]
        })];
        let decoded = decode_opencode(&raw);
        let mut output = decoded
            .messages
            .iter()
            .map(|message| message.ck.clone())
            .collect::<Vec<_>>();

        *output[0].content_mut()[1].kind_mut() = BlockKind::Text {
            text: "§18240§ answer".to_string(),
        };
        *output[0].content_mut()[2].kind_mut() = BlockKind::Text {
            text: String::new(),
        };

        let served =
            encode_opencode_with_session(&output, &decoded.sidecar, Some("ses_live"), None);
        assert_eq!(served[0]["parts"][1]["text"], "§18240§ answer");
        assert_eq!(served[0]["parts"][2]["type"], "text");
        assert_eq!(served[0]["parts"][2]["text"], "");
    }
}
