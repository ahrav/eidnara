//! Closed, loss-aware conversion between Pi `AgentMessage` rows and canonical wire messages.
//!
//! A row is `{"id": <plugin-assigned id>, "message": <AgentMessage>}`. The id is the persisted Pi
//! entry id for a host message and a reserved id for a synthetic entry. The role set is closed to
//! Pi 0.80.2's `AgentMessage` union; a row with any other role makes `decode_pi_rows` decline
//! the window. Decoding keeps every row in a sidecar, so encoding replays untouched rows as their
//! exact retained values, updates edited blocks in place, and removes deleted native parts.
//! Encoding is a pure function of its inputs, so identical frozen messages re-encode to identical
//! rows.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use super::json::{media_kind, opaque_arc, set_string, set_value, string_field, synth_tool_id};
use crate::injection::SYNTHETIC_TIMESTAMP;
use crate::wire::{
    BlockKind, HarnessMeta, IngressMessage, MediaBlock, MediaKind, MessageOrigin, OpaqueBlock,
    OutputKind, ProviderExtras, ResultBlock, ResultBlockKind, ToolOutput, WireBlock, WireMessage,
};

use super::sidecar::{
    BlockMeta, DecodeSidecar, DecodedHarnessMessages, HarnessMessageMeta, MatchedBlockMetas,
    block_is_unchanged, decoded_block_fingerprint, equals_decoded_block,
    has_stamped_block_identity, match_block_metas, stable_hash_prefix, stamp_block_identity,
};

const HARNESS: &str = "pi";

/// Prefix of every reserved row id. Pi entry ids are lowercase hexadecimal, optionally
/// hyphenated, so no host id contains the `:` this prefix carries.
pub(crate) const PI_RESERVED_ID_PREFIX: &str = "eidnara:";

/// Text Pi 0.80.2 renders around a compaction summary (`COMPACTION_SUMMARY_PREFIX` and
/// `COMPACTION_SUMMARY_SUFFIX` in `core/messages.js`).
const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";
/// Text Pi 0.80.2 renders around a branch summary (`BRANCH_SUMMARY_PREFIX` and
/// `BRANCH_SUMMARY_SUFFIX`).
const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";
const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

/// The closed Pi 0.80.2 `AgentMessage` role set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PiRole {
    User,
    Assistant,
    ToolResult,
    BashExecution,
    Custom,
    BranchSummary,
    CompactionSummary,
}

impl PiRole {
    pub(crate) const ALL: [Self; 7] = [
        Self::User,
        Self::Assistant,
        Self::ToolResult,
        Self::BashExecution,
        Self::Custom,
        Self::BranchSummary,
        Self::CompactionSummary,
    ];

    pub(crate) fn parse(role: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.wire_id() == role)
    }

    pub(crate) const fn wire_id(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::ToolResult => "toolResult",
            Self::BashExecution => "bashExecution",
            Self::Custom => "custom",
            Self::BranchSummary => "branchSummary",
            Self::CompactionSummary => "compactionSummary",
        }
    }

    /// Roles the plugin builds from a non-message session entry; their rows carry reserved ids.
    pub(crate) const fn is_synthetic_entry(self) -> bool {
        matches!(
            self,
            Self::Custom | Self::BranchSummary | Self::CompactionSummary
        )
    }

    /// The canonical role Pi's `convertToLlm` gives the message.
    const fn wire_role(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::ToolResult => "tool",
            Self::User
            | Self::BashExecution
            | Self::Custom
            | Self::BranchSummary
            | Self::CompactionSummary => "user",
        }
    }
}

/// Why [`decode_pi_rows`] declined a window. Every reason refuses the whole pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PiDecline {
    /// The row is not an object with a non-empty string `id` and an object `message` whose
    /// `role` is a string.
    MalformedRow { index: usize },
    /// The row's role is outside the closed set.
    UnknownRole { index: usize, role: String },
    /// A host-message row carries a reserved id, or a synthetic-entry row carries a host id.
    IdSpace { index: usize, role: PiRole },
    /// The row's id is not the `mid` of the CK message at its position.
    IdMismatch { index: usize },
    /// The native window and the CK window hold different message counts.
    LengthMismatch { rows: usize, messages: usize },
}

impl std::error::Error for PiDecline {}

impl fmt::Display for PiDecline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedRow { index } => write!(
                f,
                "pi native message {index} is not an {{id, message: {{role}}}} row"
            ),
            Self::UnknownRole { index, role } => write!(
                f,
                "pi native message {index} has role {role:?}, outside the closed AgentMessage role set"
            ),
            Self::IdSpace { index, role } => {
                let space = if role.is_synthetic_entry() {
                    "a reserved id"
                } else {
                    "a host entry id"
                };
                write!(
                    f,
                    "pi native message {index} ({}) must carry {space}",
                    role.wire_id()
                )
            }
            Self::IdMismatch { index } => write!(
                f,
                "pi native message {index} does not carry the mid of messages[{index}]"
            ),
            Self::LengthMismatch { rows, messages } => write!(
                f,
                "native_messages holds {rows} pi rows for {messages} messages"
            ),
        }
    }
}

/// Returns whether `id` is in the reserved id space.
pub(crate) fn is_reserved_id(id: &str) -> bool {
    id.starts_with(PI_RESERVED_ID_PREFIX)
}

/// Returns a row's id, the recipe key of a Pi native value.
pub(crate) fn row_id(row: &Value) -> Option<&str> {
    row.get("id").and_then(Value::as_str)
}

/// Decodes a window of Pi rows, or declines it.
///
/// A message's ordinal is its window position, `index + 1`. Each decoded message's mid is its
/// row id. The sidecar retains each row through an `Arc` clone. Decoded blocks carry no identity
/// stamp, and [`encode_pi_rows`] fingerprints a message's decoded blocks when it aligns them.
pub(crate) fn decode_pi_rows(rows: &[Arc<Value>]) -> Result<DecodedHarnessMessages, PiDecline> {
    decode_rows(rows, false)
}

/// Decodes like [`decode_pi_rows`], then fingerprints every block meta and stamps every decoded
/// block with its origin and fingerprint.
#[cfg(test)]
pub(crate) fn decode_pi_rows_stamped(
    rows: &[Arc<Value>],
) -> Result<DecodedHarnessMessages, PiDecline> {
    decode_rows(rows, true)
}

fn decode_rows(rows: &[Arc<Value>], stamp: bool) -> Result<DecodedHarnessMessages, PiDecline> {
    let mut sidecar = DecodeSidecar::new(HARNESS);
    let mut decoded = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let (mid, message, role) = classify_row(index, row)?;
        let ordinal = (index as u64).saturating_add(1);
        let mut content = Vec::new();
        let mut block_metas = Vec::new();
        match role {
            PiRole::User | PiRole::Custom => {
                decode_user_message(message, &mut content, &mut block_metas);
            }
            PiRole::Assistant => {
                decode_assistant_message(message, ordinal, &mut content, &mut block_metas);
            }
            PiRole::ToolResult => {
                decode_tool_result_message(message, &mut content, &mut block_metas);
            }
            PiRole::BashExecution => {
                if message.get("excludeFromContext").and_then(Value::as_bool) != Some(true) {
                    push_message_text(&mut content, &mut block_metas, bash_execution_text(message));
                }
            }
            PiRole::BranchSummary => push_message_text(
                &mut content,
                &mut block_metas,
                format!(
                    "{BRANCH_SUMMARY_PREFIX}{}{BRANCH_SUMMARY_SUFFIX}",
                    string_field(message, "summary").unwrap_or_default()
                ),
            ),
            PiRole::CompactionSummary => push_message_text(
                &mut content,
                &mut block_metas,
                format!(
                    "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                    string_field(message, "summary").unwrap_or_default()
                ),
            ),
        }
        if stamp {
            for (block, block_meta) in content.iter_mut().zip(&mut block_metas) {
                let fingerprint = decoded_block_fingerprint(block);
                stamp_block_identity(
                    block,
                    block_meta.block_index,
                    block_meta.native_index.unwrap_or_default(),
                    &fingerprint,
                );
                block_meta.content_fingerprint = Some(fingerprint);
            }
        }
        let origin = if role == PiRole::Assistant {
            pi_origin(message)
        } else {
            None
        };
        let ck = WireMessage::from_parts(
            role.wire_role(),
            content,
            origin,
            ProviderExtras::new(),
            HarnessMeta {
                harness_id: Some(mid.to_string()),
                ordinal: Some(ordinal),
                errored: role == PiRole::Assistant
                    && message.get("stopReason").and_then(Value::as_str) == Some("error"),
                created_at_ms: message.get("timestamp").and_then(Value::as_i64),
                ..Default::default()
            },
        );
        decoded.push(IngressMessage {
            mid: mid.to_string(),
            ordinal,
            ck,
        });
        sidecar.remember_message(
            mid.to_string(),
            HarnessMessageMeta {
                mid: mid.to_string(),
                ordinal,
                role: role.wire_id().to_string(),
                raw: Arc::clone(row),
                stable_key: None,
                blocks: block_metas,
            },
        );
    }
    Ok(DecodedHarnessMessages {
        messages: decoded,
        boundary: None,
        sidecar,
    })
}

/// Validates every row of a window against the CK window's mids, without decoding its blocks.
pub(crate) fn check_pi_rows<'a>(
    rows: &[Arc<Value>],
    mids: impl ExactSizeIterator<Item = &'a str>,
) -> Result<(), PiDecline> {
    if rows.len() != mids.len() {
        return Err(PiDecline::LengthMismatch {
            rows: rows.len(),
            messages: mids.len(),
        });
    }
    rows.iter()
        .zip(mids)
        .enumerate()
        .try_for_each(|(index, (row, mid))| {
            let (id, _, _) = classify_row(index, row)?;
            if id == mid {
                Ok(())
            } else {
                Err(PiDecline::IdMismatch { index })
            }
        })
}

fn classify_row(index: usize, row: &Value) -> Result<(&str, &Value, PiRole), PiDecline> {
    let malformed = || PiDecline::MalformedRow { index };
    let id = row_id(row)
        .filter(|id| !id.is_empty())
        .ok_or_else(malformed)?;
    let message = row
        .get("message")
        .filter(|message| message.is_object())
        .ok_or_else(malformed)?;
    let role_name = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(malformed)?;
    let role = PiRole::parse(role_name).ok_or_else(|| PiDecline::UnknownRole {
        index,
        role: role_name.to_string(),
    })?;
    if role.is_synthetic_entry() != is_reserved_id(id) {
        return Err(PiDecline::IdSpace { index, role });
    }
    Ok((id, message, role))
}

/// Pi 0.80.2 `bashExecutionToText`.
fn bash_execution_text(message: &Value) -> String {
    let command = string_field(message, "command").unwrap_or_default();
    let mut text = format!("Ran `{command}`\n");
    match string_field(message, "output").filter(|output| !output.is_empty()) {
        Some(output) => text.push_str(&format!("```\n{output}\n```")),
        None => text.push_str("(no output)"),
    }
    let exit_code = message.get("exitCode").and_then(Value::as_i64);
    if message.get("cancelled").and_then(Value::as_bool) == Some(true) {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(code) = exit_code.filter(|code| *code != 0) {
        text.push_str(&format!("\n\nCommand exited with code {code}"));
    }
    if message.get("truncated").and_then(Value::as_bool) == Some(true)
        && let Some(path) = string_field(message, "fullOutputPath").filter(|path| !path.is_empty())
    {
        text.push_str(&format!("\n\n[Output truncated. Full output: {path}]"));
    }
    text
}

/// Encodes canonical messages as Pi rows, reusing retained rows matched by mid.
///
/// An untouched retained message replays its retained row `Arc`, as does any retained message named
/// in `mutation_exempt_mids`. An edited user, custom, or assistant message keeps its native parts and
/// updates the edited ones; an edited tool result rewrites its content. An edited message of a role
/// whose native shape has no content array becomes the `user` message Pi's `convertToLlm` would
/// build, under the same id. A message without a retained row is built in Pi's current shape; a
/// synthetic or unidentified one takes a reserved id derived from its encoded message, suffixed
/// with its occurrence when an equal message earlier in the output took that id. A retained
/// message whose content is empty after editing is omitted. `decoded` is the [`decode_pi_rows`]
/// output for the retained rows.
pub(crate) fn encode_pi_rows(
    messages: &[WireMessage],
    decoded: &DecodedHarnessMessages,
    mutation_exempt_mids: &[&str],
) -> Vec<Arc<Value>> {
    let mut synthetic_ids = HashMap::<String, usize>::new();
    messages
        .iter()
        .filter_map(|message| {
            let meta = message
                .meta
                .harness_id
                .as_deref()
                .and_then(|mid| decoded.sidecar.message_by_mid(mid));
            match meta {
                Some(meta) if mutation_exempt_mids.contains(&meta.mid.as_str()) => {
                    Some(Arc::clone(&meta.raw))
                }
                Some(meta) => encode_with_meta(message, meta, decoded_blocks(decoded, meta)),
                None => Some(Arc::new(encode_new_row(message, &mut synthetic_ids))),
            }
        })
        .collect()
}

fn decoded_blocks<'a>(
    decoded: &'a DecodedHarnessMessages,
    meta: &HarnessMessageMeta,
) -> &'a [WireBlock] {
    usize::try_from(meta.ordinal)
        .ok()
        .and_then(|ordinal| ordinal.checked_sub(1))
        .and_then(|index| decoded.messages.get(index))
        .filter(|message| message.mid == meta.mid)
        .map_or(&[], |message| message.ck.content())
}

fn decode_user_message(
    message: &Value,
    content: &mut Vec<WireBlock>,
    block_metas: &mut Vec<BlockMeta>,
) {
    match message.get("content") {
        Some(Value::String(text)) => push_message_text(content, block_metas, text.clone()),
        Some(Value::Array(parts)) => {
            for (part_index, part) in parts.iter().enumerate() {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = string_field(part, "text").unwrap_or_default();
                        let block = block_with_pi_extras(
                            BlockKind::Text { text },
                            pi_extras_for_text(part),
                        );
                        push_block(content, block_metas, block, part_index, part, "text");
                    }
                    Some("image") => {
                        let block = WireBlock::bare(BlockKind::Media(pi_media_from_part(part)));
                        push_block(content, block_metas, block, part_index, part, "image");
                    }
                    Some(other) => {
                        let block = opaque_block(other, part.clone(), None);
                        push_block(content, block_metas, block, part_index, part, other);
                    }
                    None => {
                        let block = opaque_block("unknown", part.clone(), None);
                        push_block(content, block_metas, block, part_index, part, "unknown");
                    }
                }
            }
        }
        _ => {}
    }
}

fn decode_assistant_message(
    message: &Value,
    ordinal: u64,
    content: &mut Vec<WireBlock>,
    block_metas: &mut Vec<BlockMeta>,
) {
    let Some(parts) = message.get("content").and_then(Value::as_array) else {
        return;
    };
    for (part_index, part) in parts.iter().enumerate() {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = string_field(part, "text").unwrap_or_default();
                let block =
                    block_with_pi_extras(BlockKind::Text { text }, pi_extras_for_text(part));
                push_block(content, block_metas, block, part_index, part, "text");
            }
            Some("thinking") => {
                let redacted = part
                    .get("redacted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if redacted {
                    let data = string_field(part, "thinkingSignature")
                        .or_else(|| string_field(part, "thinking"))
                        .unwrap_or_default();
                    let block = WireBlock::bare(BlockKind::RedactedReasoning { data });
                    push_block(
                        content,
                        block_metas,
                        block,
                        part_index,
                        part,
                        "redacted_reasoning",
                    );
                } else {
                    let text = string_field(part, "thinking").unwrap_or_default();
                    let signature = string_field(part, "thinkingSignature");
                    let block = WireBlock::bare(BlockKind::Reasoning { text, signature });
                    push_block(content, block_metas, block, part_index, part, "reasoning");
                }
            }
            Some("toolCall") => {
                let input = part.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let native_id = string_field(part, "id")
                    .or_else(|| string_field(part, "callId"))
                    .unwrap_or_else(|| {
                        synth_tool_id(ordinal, part_index, &tool_name(part), &input)
                    });
                let (canonical_id, item_id) = canonical_tool_id(&native_id);
                let extras = pi_extras_for_tool(part, item_id.as_deref(), &native_id);
                let block = WireBlock::with_provider_extras(
                    BlockKind::ToolCall {
                        id: canonical_id,
                        name: tool_name(part),
                        input,
                        provider_executed: false,
                    },
                    extras,
                );
                push_block(content, block_metas, block, part_index, part, "tool_call");
                if let Some(last) = block_metas.last_mut() {
                    last.native_id = Some(native_id);
                    last.item_id = item_id;
                }
            }
            Some(other) => {
                let block = opaque_block(other, part.clone(), opaque_arc(part));
                push_block(content, block_metas, block, part_index, part, other);
            }
            None => {
                let block = opaque_block("unknown", part.clone(), None);
                push_block(content, block_metas, block, part_index, part, "unknown");
            }
        }
    }
}

fn decode_tool_result_message(
    message: &Value,
    content: &mut Vec<WireBlock>,
    block_metas: &mut Vec<BlockMeta>,
) {
    let native_id = string_field(message, "toolCallId").unwrap_or_else(|| "tool".to_string());
    let (canonical_id, item_id) = canonical_tool_id(&native_id);
    let tool_name = string_field(message, "toolName").unwrap_or_else(|| "tool".to_string());
    let is_error = message
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let output = pi_tool_result_output(message, is_error);
    let mut extras = ProviderExtras::new();
    if let Some(item_id) = item_id.as_deref() {
        insert_pi_extra(&mut extras, "itemId", Value::String(item_id.to_string()));
        insert_pi_extra(
            &mut extras,
            "nativeToolCallId",
            Value::String(native_id.clone()),
        );
    }
    let block = WireBlock::with_provider_extras(
        BlockKind::ToolResult {
            id: canonical_id,
            tool_name,
            output,
            provider_executed: false,
        },
        extras,
    );
    push_block(content, block_metas, block, 0, &Value::Null, "tool_result");
    if let Some(last) = block_metas.last_mut() {
        last.native_id = Some(native_id);
        last.item_id = item_id;
    }
}

/// Pushes a text block decoded from the whole message rather than from one content part.
fn push_message_text(content: &mut Vec<WireBlock>, block_metas: &mut Vec<BlockMeta>, text: String) {
    let block = WireBlock::bare(BlockKind::Text { text });
    push_block(content, block_metas, block, 0, &Value::Null, "text");
}

fn push_block(
    content: &mut Vec<WireBlock>,
    block_metas: &mut Vec<BlockMeta>,
    block: WireBlock,
    native_index: usize,
    raw: &Value,
    kind: &str,
) {
    block_metas.push(BlockMeta {
        block_index: content.len(),
        kind: kind.to_string(),
        native_index: Some(native_index),
        native_id: string_field(raw, "id").or_else(|| string_field(raw, "toolCallId")),
        item_id: None,
        content_fingerprint: None,
        raw: Value::Null,
    });
    content.push(block);
}

fn encode_with_meta(
    msg: &WireMessage,
    meta: &HarnessMessageMeta,
    decoded: &[WireBlock],
) -> Option<Arc<Value>> {
    if replays_retained_row(msg, meta, decoded) {
        return Some(Arc::clone(&meta.raw));
    }
    let fingerprinted;
    let block_metas = if meta
        .blocks
        .iter()
        .all(|block_meta| block_meta.content_fingerprint.is_some())
    {
        &meta.blocks
    } else {
        fingerprinted = fingerprint_block_metas(&meta.blocks, decoded);
        &fingerprinted
    };
    let matched_metas = match_block_metas(msg.content(), block_metas, block_matches_meta);
    let unchanged = msg.content().len() == block_metas.len()
        && msg
            .content()
            .iter()
            .zip(&matched_metas.by_block)
            .all(|(block, matched)| matched.is_some_and(|meta| block_is_unchanged(block, meta)));
    if unchanged {
        return Some(Arc::clone(&meta.raw));
    }
    if msg.content().is_empty() {
        return None;
    }
    let mut row = meta.raw.as_ref().clone();
    let message = row.get_mut("message")?;
    match PiRole::parse(&meta.role)? {
        PiRole::ToolResult => {
            let matched = msg
                .content()
                .iter()
                .zip(&matched_metas.by_block)
                .find(|(block, _)| matches!(block.kind(), BlockKind::ToolResult { .. }))
                .map(|(_, matched)| matched.is_some())?;
            update_tool_result_message(message, msg, matched);
        }
        PiRole::User | PiRole::Custom | PiRole::Assistant => {
            update_pi_message_content(message, msg, &matched_metas);
        }
        PiRole::BashExecution | PiRole::BranchSummary | PiRole::CompactionSummary => {
            let timestamp = message.get("timestamp").cloned().unwrap_or(Value::from(0));
            *message = json!({
                "role": "user",
                "content": msg.content().iter().map(render_block_as_content_part).collect::<Vec<_>>(),
                "timestamp": timestamp,
            });
        }
    }
    Some(Arc::new(row))
}

/// Fills each missing fingerprint from the decoded block at the meta's index.
fn fingerprint_block_metas(block_metas: &[BlockMeta], decoded: &[WireBlock]) -> Vec<BlockMeta> {
    block_metas
        .iter()
        .map(|block_meta| BlockMeta {
            content_fingerprint: block_meta.content_fingerprint.clone().or_else(|| {
                decoded
                    .get(block_meta.block_index)
                    .map(decoded_block_fingerprint)
            }),
            ..block_meta.clone()
        })
        .collect()
}

fn replays_retained_row(
    msg: &WireMessage,
    meta: &HarnessMessageMeta,
    decoded: &[WireBlock],
) -> bool {
    msg.content().len() == meta.blocks.len()
        && msg
            .content()
            .iter()
            .zip(&meta.blocks)
            .all(|(block, block_meta)| {
                !has_stamped_block_identity(block)
                    && block_matches_meta(block, block_meta)
                    && (decoded
                        .get(block_meta.block_index)
                        .is_some_and(|decoded| equals_decoded_block(block, decoded))
                        || block_is_unchanged(block, block_meta))
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
            meta.kind == "tool_call"
                && meta.native_id.as_deref().is_none_or(|native| {
                    let (canonical, _) = canonical_tool_id(native);
                    canonical == id.as_str()
                })
        }
        BlockKind::ToolResult { id, .. } => {
            meta.kind == "tool_result"
                && meta.native_id.as_deref().is_none_or(|native| {
                    let (canonical, _) = canonical_tool_id(native);
                    canonical == id.as_str()
                })
        }
        BlockKind::Media(_) => meta.kind == "image",
        BlockKind::Opaque(opaque) => meta.kind == opaque.kind,
    }
}

fn update_pi_message_content(
    message: &mut Value,
    msg: &WireMessage,
    matched_metas: &MatchedBlockMetas<'_>,
) {
    if msg.content().len() == 1
        && message.get("content").is_some_and(Value::is_string)
        && let Some(block) = msg.content().first()
        && let BlockKind::Text { text } = block.kind()
    {
        set_value(message, "content", Value::String(text.clone()));
        return;
    }

    let mut parts = match message.get("content") {
        Some(Value::Array(parts)) => parts.clone(),
        Some(Value::String(text)) if !text.is_empty() => {
            vec![json!({ "type": "text", "text": text })]
        }
        _ => Vec::new(),
    };
    for (block, block_meta) in msg.content().iter().zip(&matched_metas.by_block) {
        if let Some(part_index) = block_meta.and_then(|block_meta| block_meta.native_index)
            && let Some(part) = parts.get_mut(part_index)
        {
            if block_meta.is_some_and(|meta| block_is_unchanged(block, meta)) {
                continue;
            }
            if !matches!(
                block.kind(),
                BlockKind::Reasoning { .. } | BlockKind::RedactedReasoning { .. }
            ) {
                update_content_part(part, block);
            }
            continue;
        }
        parts.push(render_block_as_content_part(block));
    }
    parts = matched_metas.remove_unretained_native_parts(parts);
    set_value(message, "content", Value::Array(parts));
}

fn update_tool_result_message(raw: &mut Value, msg: &WireMessage, preserve_existing_id: bool) {
    let Some(block) = msg
        .content()
        .iter()
        .find(|block| matches!(block.kind(), BlockKind::ToolResult { .. }))
    else {
        return;
    };
    if let BlockKind::ToolResult {
        id,
        tool_name,
        output,
        ..
    } = block.kind()
    {
        let existing = preserve_existing_id
            .then(|| string_field(raw, "toolCallId"))
            .flatten();
        set_string(raw, "role", "toolResult");
        set_string(
            raw,
            "toolCallId",
            &native_tool_id(id, &block.provider_extras, existing),
        );
        set_string(raw, "toolName", tool_name);
        let is_error = matches!(
            output.kind,
            OutputKind::ErrorText { .. }
                | OutputKind::ErrorJson { .. }
                | OutputKind::ErrorContent { .. }
                | OutputKind::ExecutionDenied { .. }
        );
        set_value(raw, "isError", Value::Bool(is_error));
        set_value(
            raw,
            "content",
            Value::Array(render_tool_result_content(output)),
        );
    }
}

fn update_content_part(part: &mut Value, block: &WireBlock) {
    match block.kind() {
        BlockKind::Text { text } => {
            set_string(part, "type", "text");
            set_string(part, "text", text);
            if let Some(sig) = block
                .provider_extras
                .get(HARNESS)
                .and_then(|ns| ns.get("textSignature"))
                .and_then(Value::as_str)
            {
                set_string(part, "textSignature", sig);
            }
        }
        BlockKind::Reasoning { text, signature } => {
            set_string(part, "type", "thinking");
            set_string(part, "thinking", text);
            if let Some(signature) = signature {
                set_string(part, "thinkingSignature", signature);
            }
        }
        BlockKind::RedactedReasoning { data } => {
            set_string(part, "type", "thinking");
            set_string(part, "thinking", "");
            set_string(part, "thinkingSignature", data);
            set_value(part, "redacted", Value::Bool(true));
        }
        BlockKind::ToolCall {
            id, name, input, ..
        } => {
            let existing = string_field(part, "id");
            set_string(part, "type", "toolCall");
            set_string(
                part,
                "id",
                &native_tool_id(id, &block.provider_extras, existing),
            );
            set_string(part, "name", name);
            set_value(part, "arguments", input.clone());
            if let Some(sig) = block
                .provider_extras
                .get(HARNESS)
                .and_then(|ns| ns.get("thoughtSignature"))
                .and_then(Value::as_str)
            {
                set_string(part, "thoughtSignature", sig);
            }
        }
        BlockKind::Media(media) => {
            *part = render_media_part(media);
        }
        BlockKind::Opaque(opaque) => {
            *part = opaque.raw.clone();
        }
        BlockKind::ToolResult { .. } => {
            *part = render_block_as_content_part(block);
        }
    }
}

fn encode_new_row(msg: &WireMessage, synthetic_ids: &mut HashMap<String, usize>) -> Value {
    let message = if msg.role == "tool" {
        let mut raw =
            json!({ "role": "toolResult", "content": [], "timestamp": SYNTHETIC_TIMESTAMP });
        update_tool_result_message(&mut raw, msg, false);
        raw
    } else {
        let content: Vec<Value> = msg
            .content()
            .iter()
            .map(render_block_as_content_part)
            .collect();
        if msg.role == "assistant" {
            let stop_reason = if content.iter().any(|part| part["type"] == "toolCall") {
                "toolUse"
            } else {
                "stop"
            };
            json!({
                "role": "assistant",
                "content": content,
                "api": msg.origin.as_ref().map(|o| o.api.as_str()).unwrap_or(""),
                "provider": msg.origin.as_ref().map(|o| o.provider.as_str()).unwrap_or(""),
                "model": msg.origin.as_ref().map(|o| o.model.as_str()).unwrap_or(""),
                "usage": {
                    "input": 0,
                    "output": 0,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                    "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
                },
                "stopReason": stop_reason,
                "timestamp": SYNTHETIC_TIMESTAMP,
            })
        } else {
            json!({ "role": "user", "content": content, "timestamp": SYNTHETIC_TIMESTAMP })
        }
    };
    let id = msg
        .meta
        .harness_id
        .clone()
        .filter(|id| !msg.meta.synthetic && !id.is_empty())
        .unwrap_or_else(|| {
            let id = format!(
                "{PI_RESERVED_ID_PREFIX}synthetic:{}",
                stable_hash_prefix(&message, 24)
            );
            let occurrence = synthetic_ids.entry(id.clone()).or_default();
            *occurrence += 1;
            match *occurrence {
                1 => id,
                n => format!("{id}:{n}"),
            }
        });
    json!({ "id": id, "message": message })
}

fn render_block_as_content_part(block: &WireBlock) -> Value {
    match block.kind() {
        BlockKind::Text { text } => {
            let mut part = json!({ "type": "text", "text": text });
            if let Some(sig) = block
                .provider_extras
                .get(HARNESS)
                .and_then(|ns| ns.get("textSignature"))
                .and_then(Value::as_str)
            {
                set_string(&mut part, "textSignature", sig);
            }
            part
        }
        BlockKind::Reasoning { text, signature } => {
            let mut part = json!({ "type": "thinking", "thinking": text });
            if let Some(signature) = signature {
                set_string(&mut part, "thinkingSignature", signature);
            }
            part
        }
        BlockKind::RedactedReasoning { data } => {
            json!({ "type": "thinking", "thinking": "", "thinkingSignature": data, "redacted": true })
        }
        BlockKind::ToolCall {
            id, name, input, ..
        } => {
            let mut part = json!({
                "type": "toolCall",
                "id": native_tool_id(id, &block.provider_extras, None),
                "name": name,
                "arguments": input,
            });
            if let Some(sig) = block
                .provider_extras
                .get(HARNESS)
                .and_then(|ns| ns.get("thoughtSignature"))
                .and_then(Value::as_str)
            {
                set_string(&mut part, "thoughtSignature", sig);
            }
            part
        }
        BlockKind::ToolResult { output, .. } => json!({
            "type": "text",
            "text": output_text(output),
        }),
        BlockKind::Media(media) => render_media_part(media),
        BlockKind::Opaque(opaque) => opaque.raw.clone(),
    }
}

fn pi_origin(message: &Value) -> Option<MessageOrigin> {
    Some(MessageOrigin {
        api: string_field(message, "api")?,
        provider: string_field(message, "provider")?,
        model: string_field(message, "model")?,
    })
}

fn canonical_tool_id(native_id: &str) -> (String, Option<String>) {
    if let Some((call_id, item_id)) = native_id.split_once('|') {
        (call_id.to_string(), Some(item_id.to_string()))
    } else {
        (native_id.to_string(), None)
    }
}

fn native_tool_id(id: &str, extras: &ProviderExtras, existing: Option<String>) -> String {
    if let Some(existing) = existing
        && !existing.is_empty()
    {
        return existing;
    }
    if let Some(native) = extras
        .get(HARNESS)
        .and_then(|ns| ns.get("nativeToolCallId"))
        .and_then(Value::as_str)
    {
        return native.to_string();
    }
    if let Some(item_id) = extras
        .get(HARNESS)
        .and_then(|ns| ns.get("itemId"))
        .and_then(Value::as_str)
    {
        return format!("{id}|{item_id}");
    }
    id.to_string()
}

fn pi_extras_for_text(part: &Value) -> ProviderExtras {
    let mut extras = ProviderExtras::new();
    if let Some(signature) = string_field(part, "textSignature") {
        insert_pi_extra(&mut extras, "textSignature", Value::String(signature));
    }
    extras
}

fn pi_extras_for_tool(part: &Value, item_id: Option<&str>, native_id: &str) -> ProviderExtras {
    let mut extras = ProviderExtras::new();
    if let Some(signature) = string_field(part, "thoughtSignature") {
        insert_pi_extra(&mut extras, "thoughtSignature", Value::String(signature));
    }
    if let Some(item_id) = item_id {
        insert_pi_extra(&mut extras, "itemId", Value::String(item_id.to_string()));
    }
    insert_pi_extra(
        &mut extras,
        "nativeToolCallId",
        Value::String(native_id.to_string()),
    );
    extras
}

fn insert_pi_extra(extras: &mut ProviderExtras, key: &str, value: Value) {
    extras
        .entry(HARNESS.to_string())
        .or_default()
        .insert(key.to_string(), value);
}

fn block_with_pi_extras(kind: BlockKind, extras: ProviderExtras) -> WireBlock {
    if extras.is_empty() {
        WireBlock::bare(kind)
    } else {
        WireBlock::with_provider_extras(kind, extras)
    }
}

fn pi_tool_result_output(message: &Value, is_error: bool) -> ToolOutput {
    let Some(parts) = message.get("content").and_then(Value::as_array) else {
        return ToolOutput::bare(if is_error {
            OutputKind::ErrorText {
                text: String::new(),
            }
        } else {
            OutputKind::Text {
                text: String::new(),
            }
        });
    };

    if let [part] = parts.as_slice() {
        let only_plain_fields = part.as_object().is_some_and(|object| {
            object
                .keys()
                .all(|key| matches!(key.as_str(), "type" | "text"))
        });
        if only_plain_fields && part.get("type").and_then(Value::as_str) == Some("text") {
            let text = string_field(part, "text").unwrap_or_default();
            return ToolOutput::bare(if is_error {
                OutputKind::ErrorText { text }
            } else {
                OutputKind::Text { text }
            });
        }
    }

    let blocks = parts
        .iter()
        .map(|part| {
            let mut provider_extras = ProviderExtras::new();
            provider_extras
                .entry(HARNESS.to_string())
                .or_default()
                .insert("rawResultPart".to_string(), part.clone());
            let kind = match part.get("type").and_then(Value::as_str) {
                Some("text") => ResultBlockKind::Text {
                    text: string_field(part, "text").unwrap_or_default(),
                },
                Some("image" | "file") => ResultBlockKind::Media {
                    media: pi_media_from_part(part),
                },
                Some(other) => ResultBlockKind::Opaque {
                    opaque: OpaqueBlock {
                        source: json!({ "type": "harness", "harness": HARNESS }),
                        kind: other.to_string(),
                        raw: part.clone(),
                        arc: None,
                    },
                },
                None => ResultBlockKind::Opaque {
                    opaque: OpaqueBlock {
                        source: json!({ "type": "harness", "harness": HARNESS }),
                        kind: "unknown".to_string(),
                        raw: part.clone(),
                        arc: None,
                    },
                },
            };
            ResultBlock {
                kind,
                provider_extras,
            }
        })
        .collect();
    ToolOutput::bare(if is_error {
        OutputKind::ErrorContent { blocks }
    } else {
        OutputKind::Content { blocks }
    })
}

fn render_tool_result_content(output: &ToolOutput) -> Vec<Value> {
    match &output.kind {
        OutputKind::Text { text } | OutputKind::ErrorText { text } => {
            vec![json!({ "type": "text", "text": text })]
        }
        OutputKind::Json { value } | OutputKind::ErrorJson { value } => {
            vec![json!({ "type": "text", "text": value.to_string() })]
        }
        OutputKind::ExecutionDenied { reason } => vec![json!({
            "type": "text",
            "text": reason.clone().unwrap_or_else(|| "Execution denied".to_string())
        })],
        OutputKind::Content { blocks } | OutputKind::ErrorContent { blocks } => {
            blocks.iter().map(render_tool_result_block).collect()
        }
    }
}

fn render_tool_result_block(block: &ResultBlock) -> Value {
    let retained = block
        .provider_extras
        .get(HARNESS)
        .and_then(|namespace| namespace.get("rawResultPart"))
        .cloned();
    match &block.kind {
        ResultBlockKind::Text { text } => {
            let mut part = retained.unwrap_or_else(|| json!({ "type": "text" }));
            set_string(&mut part, "type", "text");
            set_string(&mut part, "text", text);
            part
        }
        ResultBlockKind::Media { media } => {
            let Some(mut retained) = retained else {
                return render_media_part(media);
            };
            if pi_media_from_part(&retained) == *media {
                return retained;
            }
            let fresh = render_media_part(media);
            let Some(retained_object) = retained.as_object_mut() else {
                return fresh;
            };
            for key in ["type", "mimeType", "mime", "filename", "data", "url"] {
                retained_object.remove(key);
            }
            if let Some(fresh_object) = fresh.as_object() {
                retained_object.extend(fresh_object.clone());
            }
            retained
        }
        ResultBlockKind::Opaque { opaque } => opaque.raw.clone(),
    }
}

fn output_text(output: &ToolOutput) -> String {
    match &output.kind {
        OutputKind::Text { text } | OutputKind::ErrorText { text } => text.clone(),
        OutputKind::Json { value } | OutputKind::ErrorJson { value } => value.to_string(),
        OutputKind::ExecutionDenied { reason } => reason
            .clone()
            .unwrap_or_else(|| "Execution denied".to_string()),
        OutputKind::Content { blocks } | OutputKind::ErrorContent { blocks } => blocks
            .iter()
            .filter_map(|block| match &block.kind {
                ResultBlockKind::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn pi_media_from_part(part: &Value) -> MediaBlock {
    let media_type = string_field(part, "mimeType")
        .or_else(|| string_field(part, "mime"))
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let source = if let Some(data) = string_field(part, "data") {
        json!({ "type": "data_base64", "data": data })
    } else if let Some(url) = string_field(part, "url") {
        json!({ "type": "url", "url": url })
    } else {
        json!({ "type": "opaque", "raw": part })
    };
    MediaBlock {
        kind: media_kind(&media_type),
        media_type,
        filename: string_field(part, "filename"),
        source,
    }
}

fn render_media_part(media: &MediaBlock) -> Value {
    let mut part = json!({ "type": "image", "mimeType": media.media_type });
    if media.kind != MediaKind::Image {
        set_string(&mut part, "type", "file");
    }
    if let Some(filename) = &media.filename {
        set_string(&mut part, "filename", filename);
    }
    if let Some(obj) = media.source.as_object() {
        match obj.get("type").and_then(Value::as_str) {
            Some("data_base64") => {
                if let Some(data) = obj.get("data").and_then(Value::as_str) {
                    set_string(&mut part, "data", data);
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

fn opaque_block(kind: &str, raw: Value, arc: Option<Value>) -> WireBlock {
    super::json::opaque_block(HARNESS, kind, raw, arc)
}

fn tool_name(part: &Value) -> String {
    string_field(part, "name").unwrap_or_else(|| "tool".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wraps bare messages in rows with host ids.
    fn rows(messages: &[Value]) -> Vec<Arc<Value>> {
        messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                Arc::new(json!({ "id": format!("{index:08x}"), "message": message }))
            })
            .collect()
    }

    fn decode(messages: &[Value]) -> DecodedHarnessMessages {
        decode_pi_rows_stamped(&rows(messages)).expect("closed-role window decodes")
    }

    /// Encodes and unwraps each row's message.
    fn encode(messages: &[WireMessage], decoded: &DecodedHarnessMessages) -> Vec<Value> {
        encode_pi_rows(messages, decoded, &[])
            .into_iter()
            .map(|row| row["message"].clone())
            .collect()
    }

    fn cks(decoded: &DecodedHarnessMessages) -> Vec<WireMessage> {
        decoded
            .messages
            .iter()
            .map(|message| message.ck.clone())
            .collect()
    }

    fn text_of(message: &WireMessage) -> Vec<String> {
        message
            .content()
            .iter()
            .map(|block| match block.kind() {
                BlockKind::Text { text } => text.clone(),
                other => panic!("expected text, got {other:?}"),
            })
            .collect()
    }

    /// One row per role of the closed set, with the canonical role and text Pi's `convertToLlm`
    /// gives it.
    fn closed_role_table() -> Vec<(Value, &'static str, Vec<String>)> {
        vec![
            (
                json!({ "id": "1a2b3c4d", "message": { "role": "user", "content": "hi", "timestamp": 1 } }),
                "user",
                vec!["hi".to_string()],
            ),
            (
                json!({ "id": "2b3c4d5e", "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "hello" }],
                    "api": "anthropic-messages", "provider": "anthropic", "model": "m",
                    "usage": {}, "stopReason": "stop", "timestamp": 2
                } }),
                "assistant",
                vec!["hello".to_string()],
            ),
            (
                json!({ "id": "3c4d5e6f", "message": {
                    "role": "toolResult", "toolCallId": "call-1", "toolName": "read",
                    "content": [{ "type": "text", "text": "out" }], "isError": false, "timestamp": 3
                } }),
                "tool",
                Vec::new(),
            ),
            (
                json!({ "id": "4d5e6f70", "message": {
                    "role": "bashExecution", "command": "ls", "output": "a\nb", "exitCode": 2,
                    "cancelled": false, "truncated": true, "fullOutputPath": "/tmp/out",
                    "timestamp": 4
                } }),
                "user",
                vec!["Ran `ls`\n```\na\nb\n```\n\nCommand exited with code 2\n\n[Output truncated. Full output: /tmp/out]".to_string()],
            ),
            (
                json!({ "id": "eidnara:custom:5e6f7081", "message": {
                    "role": "custom", "customType": "note", "content": "custom text",
                    "display": true, "timestamp": 5
                } }),
                "user",
                vec!["custom text".to_string()],
            ),
            (
                json!({ "id": "eidnara:branchSummary:6f708192", "message": {
                    "role": "branchSummary", "summary": "branch", "fromId": "1a2b3c4d", "timestamp": 6
                } }),
                "user",
                vec![format!("{BRANCH_SUMMARY_PREFIX}branch{BRANCH_SUMMARY_SUFFIX}")],
            ),
            (
                json!({ "id": "eidnara:compactionSummary:708192a3", "message": {
                    "role": "compactionSummary", "summary": "folded", "tokensBefore": 10, "timestamp": 7
                } }),
                "user",
                vec![format!("{COMPACTION_SUMMARY_PREFIX}folded{COMPACTION_SUMMARY_SUFFIX}")],
            ),
        ]
    }

    #[test]
    fn every_closed_role_decodes_to_its_canonical_role_and_round_trips() {
        let table = closed_role_table();
        assert_eq!(
            table
                .iter()
                .map(|(row, _, _)| row["message"]["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            PiRole::ALL.map(PiRole::wire_id),
            "the table covers the closed set in order"
        );
        let window: Vec<Arc<Value>> = table
            .iter()
            .map(|(row, _, _)| Arc::new(row.clone()))
            .collect();
        let decoded = decode_pi_rows_stamped(&window).unwrap();
        for ((row, wire_role, texts), message) in table.iter().zip(&decoded.messages) {
            assert_eq!(message.mid, row["id"].as_str().unwrap());
            assert_eq!(
                message.ck.meta.harness_id.as_deref(),
                Some(message.mid.as_str())
            );
            assert_eq!(message.ck.role, *wire_role, "{row}");
            if *wire_role != "tool" {
                assert_eq!(&text_of(&message.ck), texts, "{row}");
            }
        }
        let encoded = encode_pi_rows(&cks(&decoded), &decoded, &[]);
        let expected: Vec<Arc<Value>> =
            table.into_iter().map(|(row, _, _)| Arc::new(row)).collect();
        assert_eq!(encoded, expected);
    }

    #[test]
    fn an_unknown_role_or_malformed_row_declines_the_window() {
        let mut window: Vec<Arc<Value>> = closed_role_table()
            .into_iter()
            .map(|(row, _, _)| Arc::new(row))
            .collect();
        window.push(Arc::new(
            json!({ "id": "8192a3b4", "message": { "role": "system", "content": "x" } }),
        ));
        assert_eq!(
            decode_pi_rows_stamped(&window),
            Err(PiDecline::UnknownRole {
                index: 7,
                role: "system".to_string()
            })
        );
        let mids: Vec<String> = window
            .iter()
            .map(|row| row_id(row).unwrap().to_string())
            .collect();
        assert_eq!(
            check_pi_rows(&window, mids.iter().map(String::as_str)),
            decode_pi_rows_stamped(&window).map(drop)
        );
        assert_eq!(
            check_pi_rows(&window[..1], ["other"].into_iter()),
            Err(PiDecline::IdMismatch { index: 0 })
        );
        assert_eq!(
            check_pi_rows(&window[..1], std::iter::empty()),
            Err(PiDecline::LengthMismatch {
                rows: 1,
                messages: 0
            })
        );
        for malformed in [
            json!({ "message": { "role": "user", "content": "x" } }),
            json!({ "id": "", "message": { "role": "user", "content": "x" } }),
            json!({ "id": "a", "message": "user" }),
            json!({ "id": "a", "message": { "content": "x" } }),
            json!({ "info": { "id": "a", "role": "user" }, "parts": [] }),
        ] {
            assert_eq!(
                decode_pi_rows_stamped(&[Arc::new(malformed.clone())]),
                Err(PiDecline::MalformedRow { index: 0 }),
                "{malformed}"
            );
        }
    }

    #[test]
    fn reserved_ids_and_host_ids_stay_in_disjoint_spaces() {
        let host_with_reserved =
            json!({ "id": "eidnara:user:1", "message": { "role": "user", "content": "x" } });
        let synthetic_with_host = json!({ "id": "1a2b3c4d", "message": {
            "role": "compactionSummary", "summary": "s", "tokensBefore": 1, "timestamp": 1
        } });
        assert_eq!(
            decode_pi_rows_stamped(&[Arc::new(host_with_reserved)]),
            Err(PiDecline::IdSpace {
                index: 0,
                role: PiRole::User
            })
        );
        assert_eq!(
            decode_pi_rows_stamped(&[Arc::new(synthetic_with_host)]),
            Err(PiDecline::IdSpace {
                index: 0,
                role: PiRole::CompactionSummary
            })
        );

        let decoded = decode(&[json!({ "role": "user", "content": "host", "timestamp": 1 })]);
        let m0 = WireMessage::synthetic_user_text("<session-history>m0</session-history>");
        let m1 = WireMessage::synthetic_user_text("m1");
        let mut output = vec![m0.clone(), m1.clone()];
        output.extend(cks(&decoded));
        let first = encode_pi_rows(&output, &decoded, &[]);
        let again = encode_pi_rows(&output, &decoded, &[]);
        assert_eq!(first, again, "a re-encode yields the same rows and ids");
        let ids: Vec<&str> = first.iter().map(|row| row_id(row).unwrap()).collect();
        assert!(is_reserved_id(ids[0]) && is_reserved_id(ids[1]), "{ids:?}");
        assert_ne!(ids[0], ids[1]);
        assert_eq!(ids[2], "00000000");
        assert!(!is_reserved_id(ids[2]));

        let twice = encode_pi_rows(&[m1.clone(), m1.clone()], &decoded, &[]);
        assert_eq!(row_id(&twice[0]), Some(ids[1]));
        assert_eq!(
            row_id(&twice[1]).map(str::to_owned),
            Some(format!("{}:2", ids[1]))
        );

        // A synthetic message's id follows its content, not a harness id it may carry.
        let mut labelled = m0;
        labelled.meta.harness_id = Some("1a2b3c4d".to_string());
        assert_eq!(
            row_id(&encode_pi_rows(&[labelled], &decoded, &[])[0]),
            Some(ids[0])
        );
    }

    /// SB-E07 retired: a compaction after messages is a `compactionSummary` row whose id lives in
    /// the reserved space, so every message keeps its entry id and the window round-trips.
    #[test]
    fn a_compaction_summary_after_messages_round_trips_with_entry_ids() {
        let window = vec![
            Arc::new(
                json!({ "id": "eidnara:compactionSummary:c0ffee00", "message": {
                "role": "compactionSummary", "summary": "earlier", "tokensBefore": 100, "timestamp": 5
            } }),
            ),
            Arc::new(
                json!({ "id": "a1b2c3d4", "message": { "role": "user", "content": "kept", "timestamp": 6 } }),
            ),
            Arc::new(json!({ "id": "b2c3d4e5", "message": {
                "role": "assistant", "content": [{ "type": "text", "text": "after" }],
                "usage": {}, "stopReason": "stop", "timestamp": 7
            } })),
        ];
        let decoded = decode_pi_rows_stamped(&window).unwrap();
        assert!(decoded.boundary.is_none());
        assert_eq!(
            decoded
                .messages
                .iter()
                .map(|m| m.mid.as_str())
                .collect::<Vec<_>>(),
            ["eidnara:compactionSummary:c0ffee00", "a1b2c3d4", "b2c3d4e5"]
        );
        assert_eq!(
            decoded
                .messages
                .iter()
                .map(|m| m.ordinal)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let encoded = encode_pi_rows(&cks(&decoded), &decoded, &[]);
        assert_eq!(encoded, window);
    }

    #[test]
    fn an_edited_derived_text_role_becomes_the_user_message_pi_would_send() {
        let window = vec![Arc::new(json!({ "id": "a1b2c3d4", "message": {
            "role": "bashExecution", "command": "pwd", "output": "", "exitCode": 0,
            "cancelled": false, "truncated": false, "timestamp": 9
        } }))];
        let decoded = decode_pi_rows_stamped(&window).unwrap();
        let mut message = decoded.messages[0].ck.clone();
        assert_eq!(text_of(&message), ["Ran `pwd`\n(no output)"]);
        *message.content_mut()[0].kind_mut() = BlockKind::Text {
            text: "§4§ Ran `pwd`\n(no output)".to_string(),
        };
        assert_eq!(
            encode_pi_rows(&[message], &decoded, &[]),
            [Arc::new(json!({ "id": "a1b2c3d4", "message": {
                "role": "user",
                "content": [{ "type": "text", "text": "§4§ Ran `pwd`\n(no output)" }],
                "timestamp": 9
            } }))]
        );
    }

    #[test]
    fn an_excluded_bash_execution_carries_no_content_and_replays_its_row() {
        let window = vec![Arc::new(json!({ "id": "a1b2c3d4", "message": {
            "role": "bashExecution", "command": "secret", "output": "x", "exitCode": 0,
            "cancelled": false, "truncated": false, "excludeFromContext": true, "timestamp": 9
        } }))];
        let decoded = decode_pi_rows_stamped(&window).unwrap();
        assert!(decoded.messages[0].ck.content().is_empty());
        assert_eq!(encode_pi_rows(&cks(&decoded), &decoded, &[]), window);
    }

    #[test]
    fn a_mutation_exempt_mid_replays_its_exact_row() {
        let decoded = decode(&[json!({ "role": "user", "content": "original", "timestamp": 1 })]);
        let mut message = decoded.messages[0].ck.clone();
        *message.content_mut()[0].kind_mut() = BlockKind::Text {
            text: "edited".to_string(),
        };
        assert_eq!(
            encode_pi_rows(std::slice::from_ref(&message), &decoded, &["00000000"])[0]["message"]["content"],
            "original"
        );
        assert_eq!(
            encode_pi_rows(&[message], &decoded, &[])[0]["message"]["content"],
            "edited"
        );
    }

    #[test]
    fn untouched_row_replays_the_exact_retained_value() {
        let window = vec![Arc::new(json!({
            "id": "a1b2c3d4",
            "vendorEnvelope": { "unknown": [1, 2, 3] },
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": "unchanged",
                    "textSignature": "sig",
                    "vendorPart": { "keep": true }
                }],
                "timestamp": 12,
                "vendorMessage": "keep"
            }
        }))];
        let decoded = decode_pi_rows_stamped(&window).unwrap();
        let encoded = encode_pi_rows(&cks(&decoded), &decoded, &[]);
        assert_eq!(encoded, window);
        assert!(Arc::ptr_eq(&encoded[0], &window[0]));
    }

    fn unstamped(messages: &[WireMessage]) -> Vec<WireMessage> {
        messages
            .iter()
            .cloned()
            .map(|mut message| {
                for block in message.content_mut() {
                    block
                        .provider_extras
                        .retain(|namespace, _| namespace == HARNESS);
                    assert!(!has_stamped_block_identity(block));
                }
                message
            })
            .collect()
    }

    #[test]
    fn unstamped_messages_replay_untouched_rows_and_encode_edits_as_stamped_metas_do() {
        let golden: Value =
            serde_json::from_str(include_str!("../../testdata/codec/pi-golden.json")).unwrap();
        let rows: Vec<Arc<Value>> = golden["cases"][0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(Arc::new)
            .collect();
        let decoded = decode_pi_rows(&rows).unwrap();
        let stamped = decode_pi_rows_stamped(&rows).unwrap();
        let messages = cks(&decoded);
        assert_eq!(unstamped(&cks(&stamped)), messages);
        assert!(decoded.sidecar.messages.values().all(|meta| {
            meta.blocks
                .iter()
                .all(|block_meta| block_meta.content_fingerprint.is_none())
        }));

        let encoded = encode_pi_rows(&messages, &decoded, &[]);
        assert_eq!(encoded, rows);
        assert!(
            encoded
                .iter()
                .zip(&rows)
                .all(|(encoded, row)| Arc::ptr_eq(encoded, row))
        );

        let mut edited = messages;
        let multi_block = edited
            .iter()
            .position(|message| message.content().len() > 1)
            .unwrap();
        let BlockKind::Text { text } = edited[multi_block]
            .content_mut()
            .iter_mut()
            .map(WireBlock::kind_mut)
            .find(|kind| matches!(kind, BlockKind::Text { .. }))
            .unwrap()
        else {
            unreachable!()
        };
        text.push_str(" (edited)");
        let tool_result = edited
            .iter()
            .position(|message| message.role == "tool")
            .unwrap();
        let BlockKind::ToolResult { output, .. } = edited[tool_result].content_mut()[0].kind_mut()
        else {
            unreachable!()
        };
        *output = ToolOutput::bare(OutputKind::Text {
            text: "reduced".to_string(),
        });
        let last_multi_block = edited
            .iter()
            .rposition(|message| message.content().len() > 1)
            .unwrap();
        edited[last_multi_block].content_mut().remove(0);

        let encoded = encode_pi_rows(&edited, &decoded, &[]);
        assert_eq!(encoded, encode_pi_rows(&edited, &stamped, &[]));
        assert_ne!(encoded[multi_block], rows[multi_block]);
        assert!(
            encoded[multi_block]["message"]["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part["text"]
                    .as_str()
                    .is_some_and(|text| text.ends_with(" (edited)")))
        );
        assert_eq!(
            encoded[tool_result]["message"]["content"],
            json!([{ "type": "text", "text": "reduced" }])
        );
    }

    #[test]
    fn split_pipe_tool_ids_decode_to_canonical_id_and_round_trip() {
        let raw = vec![json!({
            "role": "assistant",
            "content": [{
                "type": "toolCall",
                "id": "call-1|item-9",
                "name": "lookup",
                "arguments": { "q": "x" },
                "thoughtSignature": "sig"
            }],
            "api": "responses",
            "provider": "openai",
            "model": "gpt-test",
            "responseId": "resp-1",
            "usage": {},
            "stopReason": "toolUse",
            "timestamp": 7
        })];
        let decoded = decode(&raw);
        let block = &decoded.messages[0].ck.content()[0];
        assert!(matches!(block.kind(), BlockKind::ToolCall { id, .. } if id == "call-1"));
        assert_eq!(
            block.provider_extras[HARNESS]["itemId"],
            Value::String("item-9".to_string())
        );
        assert_eq!(encode(&[decoded.messages[0].ck.clone()], &decoded), raw);
    }

    #[test]
    fn adjacent_tool_deletion_matches_the_surviving_native_id() {
        let raw = vec![json!({
            "role": "assistant",
            "content": [
                { "type": "text", "text": "head" },
                { "type": "toolCall", "id": "call-a|item-a", "name": "first", "arguments": { "a": 1 } },
                { "type": "toolCall", "id": "call-b|item-b", "name": "second", "arguments": { "b": 2 } },
                { "type": "text", "text": "tail" }
            ],
            "api": "responses",
            "provider": "openai",
            "model": "gpt-test",
            "responseId": "resp-tools",
            "usage": {},
            "stopReason": "toolUse",
            "timestamp": 8
        })];
        let decoded = decode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(1);

        let encoded = encode(&[message], &decoded);
        let content = encoded[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0], raw[0]["content"][0]);
        assert_eq!(content[1], raw[0]["content"][2]);
        assert_eq!(content[2], raw[0]["content"][3]);
    }

    #[test]
    fn leading_deletion_does_not_shift_the_next_block_onto_the_removed_slot() {
        let raw = vec![json!({
            "role": "assistant",
            "content": [
                { "type": "toolCall", "id": "call-a", "name": "first", "arguments": {} },
                { "type": "text", "text": "survivor", "textSignature": "sig-survivor" }
            ],
            "timestamp": 9
        })];
        let decoded = decode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(0);

        let encoded = encode(&[message], &decoded);
        assert_eq!(encoded[0]["content"], json!([raw[0]["content"][1].clone()]));
    }

    #[test]
    fn duplicate_kind_adjacency_removes_only_the_deleted_text() {
        let raw = vec![json!({
            "role": "assistant",
            "content": [
                { "type": "text", "text": "A", "vendorPart": "A" },
                {
                    "type": "text",
                    "text": "DELETE",
                    "textSignature": "sig-delete",
                    "vendorPart": "delete"
                },
                { "type": "text", "text": "SURVIVE", "vendorPart": "survive" }
            ],
            "timestamp": 10
        })];
        let decoded = decode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(1);

        let encoded = encode(&[message], &decoded);
        assert_eq!(
            encoded[0]["content"],
            json!([
                { "type": "text", "text": "A", "vendorPart": "A" },
                { "type": "text", "text": "SURVIVE", "vendorPart": "survive" }
            ])
        );
    }

    #[test]
    fn mutated_text_survivor_keeps_its_own_signature_and_vendor_extras() {
        let raw = vec![json!({
            "role": "assistant",
            "content": [
                { "type": "text", "text": "A", "vendorPart": "A" },
                {
                    "type": "text",
                    "text": "DELETE",
                    "textSignature": "sig-delete",
                    "vendorPart": "delete"
                },
                {
                    "type": "text",
                    "text": "SURVIVE",
                    "textSignature": "sig-survive",
                    "vendorPart": "survive"
                }
            ],
            "timestamp": 10
        })];
        let decoded = decode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(1);
        let survivor = &mut message.content_mut()[1];
        *survivor.kind_mut() = BlockKind::Text {
            text: "§3§ SURVIVE".to_string(),
        };

        let encoded = encode(&[message], &decoded);
        assert_eq!(
            encoded[0]["content"],
            json!([
                { "type": "text", "text": "A", "vendorPart": "A" },
                {
                    "type": "text",
                    "text": "§3§ SURVIVE",
                    "textSignature": "sig-survive",
                    "vendorPart": "survive"
                }
            ])
        );
    }

    #[test]
    fn untouched_multi_text_tool_result_replays_raw_part_boundaries_and_extras() {
        let raw = vec![json!({
            "role": "toolResult",
            "toolCallId": "call-1",
            "toolName": "read",
            "isError": false,
            "content": [
                { "type": "text", "text": "a", "vendor": "keep" },
                { "type": "text", "text": "b" }
            ]
        })];
        let decoded = decode(&raw);
        let BlockKind::ToolResult { output, .. } = decoded.messages[0].ck.content()[0].kind()
        else {
            panic!("expected tool result");
        };
        assert!(matches!(output.kind, OutputKind::Content { .. }));
        assert_eq!(encode(&[decoded.messages[0].ck.clone()], &decoded), raw);
    }

    #[test]
    fn mixed_image_error_tool_result_preserves_polarity_and_part_extras_on_mutation() {
        let raw = vec![json!({
            "role": "toolResult",
            "toolCallId": "call-2",
            "toolName": "inspect",
            "isError": true,
            "content": [
                { "type": "text", "text": "failed", "vendor": "text-extra" },
                {
                    "type": "image",
                    "mimeType": "image/png",
                    "data": "aW1hZ2U=",
                    "vendor": "image-extra"
                }
            ]
        })];
        let decoded = decode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        let block = &mut message.content_mut()[0];
        let BlockKind::ToolResult { output, .. } = block.kind_mut() else {
            panic!("expected tool result");
        };
        let OutputKind::ErrorContent { blocks } = &mut output.kind else {
            panic!("mixed native error must decode as ErrorContent");
        };
        let ResultBlockKind::Text { text } = &mut blocks[0].kind else {
            panic!("expected leading text result block");
        };
        *text = "tagged failed".to_string();
        let ResultBlockKind::Media { media } = &mut blocks[1].kind else {
            panic!("expected image result block");
        };
        media.filename = Some("failure.png".to_string());

        let encoded = encode(&[message], &decoded);
        assert_eq!(encoded[0]["isError"], true);
        assert_eq!(
            encoded[0]["content"],
            json!([
                { "type": "text", "text": "tagged failed", "vendor": "text-extra" },
                {
                    "type": "image",
                    "mimeType": "image/png",
                    "filename": "failure.png",
                    "data": "aW1hZ2U=",
                    "vendor": "image-extra"
                }
            ])
        );
    }

    #[test]
    fn mixed_opaque_error_tool_result_retains_opaque_part() {
        let raw = vec![json!({
            "role": "toolResult",
            "toolCallId": "call-opaque",
            "toolName": "inspect",
            "isError": true,
            "content": [
                { "type": "text", "text": "failed" },
                { "type": "vendor-detail", "code": 17, "vendor": { "keep": true } }
            ]
        })];
        let decoded = decode(&raw);
        let BlockKind::ToolResult { output, .. } = decoded.messages[0].ck.content()[0].kind()
        else {
            panic!("expected tool result");
        };
        let OutputKind::ErrorContent { blocks } = &output.kind else {
            panic!("mixed opaque error must decode as ErrorContent");
        };
        assert!(matches!(blocks[1].kind, ResultBlockKind::Opaque { .. }));
        assert_eq!(encode(&[decoded.messages[0].ck.clone()], &decoded), raw);

        let mut message = decoded.messages[0].ck.clone();
        let result = &mut message.content_mut()[0];
        let BlockKind::ToolResult { output, .. } = result.kind_mut() else {
            panic!("expected tool result");
        };
        let OutputKind::ErrorContent { blocks } = &mut output.kind else {
            panic!("expected ErrorContent");
        };
        let ResultBlockKind::Text { text } = &mut blocks[0].kind else {
            panic!("expected leading text block");
        };
        *text = "tagged failed".to_string();
        let encoded = encode(&[message], &decoded);
        assert_eq!(encoded[0]["isError"], true);
        assert_eq!(
            encoded[0]["content"],
            json!([
                { "type": "text", "text": "tagged failed" },
                { "type": "vendor-detail", "code": 17, "vendor": { "keep": true } }
            ])
        );
    }

    #[test]
    fn empty_error_tool_result_retains_empty_content_and_error_polarity() {
        let raw = vec![json!({
            "role": "toolResult",
            "toolCallId": "call-empty",
            "toolName": "inspect",
            "isError": true,
            "content": []
        })];
        let decoded = decode(&raw);
        let BlockKind::ToolResult { output, .. } = decoded.messages[0].ck.content()[0].kind()
        else {
            panic!("expected tool result");
        };
        assert!(matches!(
            output.kind,
            OutputKind::ErrorContent { ref blocks } if blocks.is_empty()
        ));
        assert_eq!(encode(&[decoded.messages[0].ck.clone()], &decoded), raw);
    }

    #[test]
    fn frozen_deletion_replay_is_byte_stable() {
        let raw = vec![json!({
            "role": "assistant",
            "content": [
                { "type": "toolCall", "id": "call-a", "name": "first", "arguments": {} },
                { "type": "toolCall", "id": "call-b", "name": "second", "arguments": {} }
            ],
            "timestamp": 11
        })];
        let decoded = decode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().remove(0);

        let first = encode(&[message.clone()], &decoded);
        let replay = encode(&[message], &decoded);
        assert_eq!(replay, first);
        assert_eq!(first[0]["content"], json!([raw[0]["content"][1].clone()]));
    }

    #[test]
    fn deleted_tool_result_does_not_replay_the_retained_raw_entry() {
        let raw = vec![json!({
            "role": "toolResult",
            "toolCallId": "call-a",
            "toolName": "first",
            "content": [{ "type": "text", "text": "done" }],
            "isError": false,
            "timestamp": 13
        })];
        let decoded = decode(&raw);
        let mut message = decoded.messages[0].ck.clone();
        message.content_mut().clear();

        assert!(encode(&[message], &decoded).is_empty());
    }
}
