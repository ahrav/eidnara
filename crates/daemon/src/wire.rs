//! Wire-message projection into stable cache block identities.
//!
//! The cache machinery uses stable block identities instead of full wire messages.
//! This module bridges full wire messages and stable block identities.
//! The module assigns each content block a session-stable `mid#block_index` identity.
//! The module retains original messages so unreduced responses can pass them through without rebuilding.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;

use memory_store::BlockIdentity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

// The re-exported wire serializers retain the original `serde_json::Value` for pass-through.
// Pass-through must replay the retained `Value`, not round-trip through typed structs.
// Value-level replay preserves harmless future wire fields that typed round-trips could drop.
pub use memory_store::{
    BlockKind, HarnessMeta, MediaBlock, MediaKind, MessageOrigin, OpaqueBlock, OutputKind,
    ProviderExtras, ResultBlock, ResultBlockKind, ToolOutput, WireBlock, WireMessage,
};

/// One ingress message with transport identity and ordering metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngressMessage {
    pub mid: String,
    pub ordinal: u64,
    pub ck: WireMessage,
}

/// Owns shared ingress shells while preserving the message-array wire format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct IngressMessages(pub(crate) Vec<Arc<IngressMessage>>);

impl IngressMessages {
    /// Adds an owned shell without copying its contents.
    pub fn push(&mut self, message: IngressMessage) {
        self.0.push(Arc::new(message));
    }
}

impl std::ops::Deref for IngressMessages {
    type Target = Vec<Arc<IngressMessage>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for IngressMessages {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> IntoIterator for &'a IngressMessages {
    type Item = &'a Arc<IngressMessage>;
    type IntoIter = std::slice::Iter<'a, Arc<IngressMessage>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl IntoIterator for IngressMessages {
    type Item = Arc<IngressMessage>;
    type IntoIter = std::vec::IntoIter<Arc<IngressMessage>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl FromIterator<IngressMessage> for IngressMessages {
    fn from_iter<T: IntoIterator<Item = IngressMessage>>(iter: T) -> Self {
        Self(iter.into_iter().map(Arc::new).collect())
    }
}

impl FromIterator<Arc<IngressMessage>> for IngressMessages {
    fn from_iter<T: IntoIterator<Item = Arc<IngressMessage>>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// Owns a block's immutable message shell so projection clones share its backing.
/// Retaining `message` forces `Arc::make_mut` on another handle to copy the shell.
#[derive(Debug, Clone)]
pub struct SharedWireBlock {
    message: Arc<IngressMessage>,
    index: usize,
}

impl AsRef<WireBlock> for SharedWireBlock {
    fn as_ref(&self) -> &WireBlock {
        &self.message.ck.content()[self.index]
    }
}

impl std::ops::Deref for SharedWireBlock {
    type Target = WireBlock;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl PartialEq for SharedWireBlock {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

/// `FlatBlock` is the cache-stability core's internal block item.
/// `FlatBlock.bytes` measures reduction accounting, not provider-wire size.
/// The producer renders provider-wire bytes after the daemon returns wire messages.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FlatBlock {
    pub id: String,
    pub mid: String,
    pub block_index: usize,
    pub ordinal: u64,
    pub role: String,
    pub kind_tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    pub provider_executed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arc_id: Option<String>,
    pub bytes: Arc<str>,
    /// `content_hash` stores the SHA-256 of the serialized block bytes so consumers can detect content changes.
    #[serde(skip_serializing)]
    pub content_hash: [u8; 32],
    pub synthetic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_kind: Option<String>,
    #[serde(skip_serializing)]
    pub wire: SharedWireBlock,
}

/// The item view the transform cycle reads: identity, position, accounting
/// bytes, and synthetic status including projection overrides.
impl FlatBlock {
    /// `id` is `mid#block_index`, the key `identity_by_mid` and the frozen
    /// reduction set are keyed by.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The source message's ordinal, copied as supplied; the projector does not
    /// check that ordinals increase.
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }

    /// The reduction-accounting rendering, not the provider-wire bytes.
    pub fn bytes(&self) -> &str {
        &self.bytes
    }

    /// Includes projection overrides; synthetic blocks are left out of `identity_by_mid`.
    pub fn synthetic(&self) -> bool {
        self.synthetic
    }
}

/// `ProjectionState` captures projector state at each message boundary.
/// Resuming tail deltas at the saved frontier lets changed-suffix tool results pair with cached-prefix calls.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ProjectionState {
    pending_calls: BTreeMap<String, VecDeque<String>>,
    call_arcs: BTreeMap<String, String>,
}

/// Flattened message projection plus metadata needed to rebuild cached prefixes.
///
/// Block order follows message order and then content order. Each
/// `message_block_ends` entry is the exclusive block frontier for one message.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatProjection {
    pub blocks: Vec<FlatBlock>,
    pub identity_by_mid: BTreeMap<String, Vec<BlockIdentity>>,
    /// Canonical replay shells share their ownership with reattached requests.
    messages: IngressMessages,
    /// `message_block_ends` maps each transport message frontier to its flat-block end.
    /// This mapping avoids walking or serializing cached payloads.
    message_block_ends: Vec<usize>,
    /// Shared boundary states reconstruct prefixes with small `Arc` copies.
    /// Shared boundary states avoid cloning pending tool-arc maps for every retained message.
    states_after_messages: Vec<Arc<ProjectionState>>,
}

impl FlatProjection {
    pub(crate) fn message_count(&self) -> usize {
        self.message_block_ends.len()
    }

    pub(crate) fn prefix_block_count(&self, prefix_messages: usize) -> Option<usize> {
        if prefix_messages == 0 {
            return Some(0);
        }
        self.message_block_ends.get(prefix_messages - 1).copied()
    }

    /// Replay shares canonical shells. Projection ownership discards unknown
    /// top-level wire fields but retains blocks' ingress JSON.
    pub(crate) fn reattach_messages_prefix(
        &self,
        prefix_messages: usize,
    ) -> Option<IngressMessages> {
        if prefix_messages > self.message_count() || self.messages.len() != self.message_count() {
            return None;
        }

        let mut block_start = 0;
        for (message_index, message) in self.messages.iter().take(prefix_messages).enumerate() {
            let block_end = *self.message_block_ends.get(message_index)?;
            if block_end < block_start || block_end > self.blocks.len() {
                return None;
            }
            if !self.blocks[block_start..block_end].iter().enumerate().all(
                |(block_index, block)| {
                    block.mid == message.mid
                        && block.ordinal == message.ordinal
                        && block.role == message.ck.role
                        && block.block_index == block_index
                },
            ) {
                return None;
            }
            block_start = block_end;
        }
        Some(IngressMessages(self.messages[..prefix_messages].to_vec()))
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        use crate::retained_size::{
            ARC_ALLOCATION_OVERHEAD_BYTES, btree_map_allocation_bytes,
            ingress_message_retained_bytes,
        };
        use std::mem::size_of;

        let block_bytes = self
            .blocks
            .capacity()
            .saturating_mul(size_of::<FlatBlock>())
            .saturating_add(
                self.blocks
                    .iter()
                    .map(|block| {
                        block
                            .id
                            .capacity()
                            .saturating_add(block.mid.capacity())
                            .saturating_add(block.role.capacity())
                            .saturating_add(block.kind_tag.capacity())
                            .saturating_add(block.name.as_ref().map_or(0, String::capacity))
                            .saturating_add(block.file_path.as_ref().map_or(0, String::capacity))
                            .saturating_add(block.arc_id.as_ref().map_or(0, String::capacity))
                            .saturating_add(block.tool_call_id.as_ref().map_or(0, String::capacity))
                            .saturating_add(block.output_kind.as_ref().map_or(0, String::capacity))
                            .saturating_add(ARC_ALLOCATION_OVERHEAD_BYTES)
                            .saturating_add(block.bytes.len())
                    })
                    .sum::<usize>(),
            );
        let identity_bytes =
            btree_map_allocation_bytes::<String, Vec<BlockIdentity>>(self.identity_by_mid.len())
                .saturating_add(
                    self.identity_by_mid
                        .iter()
                        .map(|(mid, identities)| {
                            mid.capacity()
                                .saturating_add(
                                    identities
                                        .capacity()
                                        .saturating_mul(size_of::<BlockIdentity>()),
                                )
                                .saturating_add(
                                    identities
                                        .iter()
                                        .map(|identity| {
                                            identity.kind_tag.capacity().saturating_add(
                                                identity.byte_fingerprint.capacity(),
                                            )
                                        })
                                        .sum::<usize>(),
                                )
                        })
                        .sum::<usize>(),
                );
        let message_bytes = self
            .messages
            .capacity()
            .saturating_mul(size_of::<Arc<IngressMessage>>())
            .saturating_add(
                self.messages
                    .iter()
                    .map(|message| ingress_message_retained_bytes(message))
                    .sum::<usize>(),
            );
        let frontier_bytes = self
            .states_after_messages
            .capacity()
            .saturating_mul(size_of::<Arc<ProjectionState>>())
            .saturating_add(
                self.states_after_messages
                    .iter()
                    .map(|state| {
                        let pending_calls = btree_map_allocation_bytes::<String, VecDeque<String>>(
                            state.pending_calls.len(),
                        )
                        .saturating_add(
                            state
                                .pending_calls
                                .iter()
                                .map(|(call_id, arcs)| {
                                    call_id
                                        .capacity()
                                        .saturating_add(
                                            arcs.capacity().saturating_mul(size_of::<String>()),
                                        )
                                        .saturating_add(
                                            arcs.iter().map(String::capacity).sum::<usize>(),
                                        )
                                })
                                .sum::<usize>(),
                        );
                        let call_arcs =
                            btree_map_allocation_bytes::<String, String>(state.call_arcs.len())
                                .saturating_add(
                                    state
                                        .call_arcs
                                        .iter()
                                        .map(|(block_id, arc_id)| {
                                            block_id.capacity().saturating_add(arc_id.capacity())
                                        })
                                        .sum::<usize>(),
                                );
                        ARC_ALLOCATION_OVERHEAD_BYTES
                            .saturating_add(size_of::<ProjectionState>())
                            .saturating_add(pending_calls)
                            .saturating_add(call_arcs)
                    })
                    .sum::<usize>(),
            );

        size_of::<Self>()
            .saturating_add(block_bytes)
            .saturating_add(identity_bytes)
            .saturating_add(message_bytes)
            .saturating_add(frontier_bytes)
            .saturating_add(
                self.message_block_ends
                    .capacity()
                    .saturating_mul(size_of::<usize>()),
            )
    }

    pub(crate) fn differential_bytes(&self) -> Vec<u8> {
        let wires = self
            .blocks
            .iter()
            .map(|block| block.wire.as_ref())
            .collect::<Vec<_>>();
        serde_json::to_vec(&(&self.blocks, &self.identity_by_mid, wires))
            .expect("flat projection differential bytes must serialize")
    }
}

/// Projection failure caused by invalid identity syntax or tool-arc structure.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    #[error("message id at ordinal {ordinal} is empty")]
    EmptyMid { ordinal: u64 },
    #[error("message id contains reserved '#': {0}")]
    MidContainsReservedHash(String),
    #[error("message id {0} appears more than once")]
    DuplicateMid(String),
    #[error("unsupported wire block {kind} at {mid}#{block_index}")]
    UnsupportedBlock {
        mid: String,
        block_index: usize,
        kind: String,
    },
    #[error("tool_result {tool_call_id} at {mid}#{block_index} has no adjacent tool_call")]
    UnpairedToolResult {
        mid: String,
        block_index: usize,
        tool_call_id: String,
    },
}

/// Projects messages into stable blocks in input order.
///
/// Empty message IDs, message IDs containing `#`, message IDs that repeat,
/// unserializable blocks, and tool results without a pending call return
/// [`WireError`]. The ID rules mirror [`split_block_id`], so every projected
/// `mid#index` splits back into its parts and names exactly one message.
pub fn project_messages(messages: &[Arc<IngressMessage>]) -> Result<FlatProjection, WireError> {
    MessageProjection::new(messages).project()
}

#[cfg(test)]
pub(crate) fn project_messages_incremental(
    messages: &[Arc<IngressMessage>],
    cached: &FlatProjection,
    prefix_messages: usize,
) -> Result<FlatProjection, WireError> {
    MessageProjection::new(messages)
        .project_incremental(cached, prefix_messages)
        .map(|(projection, _)| projection)
}

/// Tracks synthetic message IDs separately from ingress messages.
pub(crate) struct MessageProjection<'a> {
    messages: &'a [Arc<IngressMessage>],
    synthetic_mids: BTreeSet<&'a str>,
}

impl<'a> MessageProjection<'a> {
    pub(crate) fn new(messages: &'a [Arc<IngressMessage>]) -> Self {
        Self {
            messages,
            synthetic_mids: BTreeSet::new(),
        }
    }

    pub(crate) fn mark_synthetic(&mut self, message: &'a IngressMessage) {
        self.synthetic_mids.insert(message.mid.as_str());
    }

    pub(crate) fn is_synthetic(&self, message: &IngressMessage) -> bool {
        message.ck.meta.synthetic || self.synthetic_mids.contains(message.mid.as_str())
    }

    pub(crate) fn live_messages(&self) -> impl DoubleEndedIterator<Item = &'a IngressMessage> + '_ {
        self.messages
            .iter()
            .map(Arc::as_ref)
            .filter(|message| !self.is_synthetic(message))
    }

    pub(crate) fn project(&self) -> Result<FlatProjection, WireError> {
        project_messages_from_state(self, FlatProjectionBuilder::default())
    }

    /// The tuple contains the projection and the number of messages reused from the cached prefix.
    ///
    /// Invalid prefix bounds, missing identities, or changed synthetic status fall
    /// back to a full projection with zero reused messages. Projection errors have
    /// the same meaning as [`project_messages`].
    pub(crate) fn project_incremental(
        &self,
        cached: &FlatProjection,
        prefix_messages: usize,
    ) -> Result<(FlatProjection, usize), WireError> {
        let messages = self.messages;
        if prefix_messages == 0
            || prefix_messages > messages.len()
            || prefix_messages > cached.message_count()
        {
            return Ok((self.project()?, 0));
        }

        let prefix_block_end = cached.message_block_ends[prefix_messages - 1];
        let mut identity_by_mid = BTreeMap::new();
        for (index, message) in messages[..prefix_messages].iter().enumerate() {
            let synthetic = self.is_synthetic(message);
            if cached.messages[index].ck.meta.synthetic != synthetic {
                return Ok((self.project()?, 0));
            }
            if synthetic {
                continue;
            }
            let Some(identities) = cached.identity_by_mid.get(&message.mid) else {
                return Ok((self.project()?, 0));
            };
            identity_by_mid.insert(message.mid.clone(), identities.clone());
        }
        let builder = FlatProjectionBuilder {
            blocks: cached.blocks[..prefix_block_end].to_vec(),
            identity_by_mid,
            messages: IngressMessages(cached.messages[..prefix_messages].to_vec()),
            message_block_ends: cached.message_block_ends[..prefix_messages].to_vec(),
            states_after_messages: cached.states_after_messages[..prefix_messages].to_vec(),
            state: cached.states_after_messages[prefix_messages - 1]
                .as_ref()
                .clone(),
        };
        Ok((project_messages_from_state(self, builder)?, prefix_messages))
    }
}

#[derive(Default)]
struct FlatProjectionBuilder {
    blocks: Vec<FlatBlock>,
    identity_by_mid: BTreeMap<String, Vec<BlockIdentity>>,
    messages: IngressMessages,
    message_block_ends: Vec<usize>,
    states_after_messages: Vec<Arc<ProjectionState>>,
    state: ProjectionState,
}

fn project_messages_from_state(
    ingress: &MessageProjection<'_>,
    mut builder: FlatProjectionBuilder,
) -> Result<FlatProjection, WireError> {
    debug_assert_eq!(builder.messages.len(), builder.message_block_ends.len());
    debug_assert_eq!(builder.messages.len(), builder.states_after_messages.len());
    // One metadata entry completes each message; its count is the suffix cursor.
    // Block ids are `mid#index`, so a repeated mid would give two messages'
    // blocks the same identities and let one message's content stand for the
    // other's. Synthetic messages take part: their block ids collide too.
    let mut seen_mids: BTreeSet<String> = builder
        .messages
        .iter()
        .map(|meta| meta.mid.clone())
        .collect();
    for msg in ingress.messages.iter().skip(builder.messages.len()) {
        if msg.mid.is_empty() {
            return Err(WireError::EmptyMid {
                ordinal: msg.ordinal,
            });
        }
        if msg.mid.contains('#') {
            return Err(WireError::MidContainsReservedHash(msg.mid.clone()));
        }
        if !seen_mids.insert(msg.mid.clone()) {
            return Err(WireError::DuplicateMid(msg.mid.clone()));
        }

        let synthetic = ingress.is_synthetic(msg);
        // Replay preserves block originals but discards unknown message fields.
        let msg = if msg.ck.original().is_none() && msg.ck.meta.synthetic == synthetic {
            Arc::clone(msg)
        } else {
            Arc::new(IngressMessage {
                mid: msg.mid.clone(),
                ordinal: msg.ordinal,
                ck: WireMessage::from_parts(
                    msg.ck.role.clone(),
                    msg.ck.content().clone(),
                    msg.ck.origin.clone(),
                    msg.ck.provider_extras.clone(),
                    HarnessMeta {
                        synthetic,
                        ..msg.ck.meta.clone()
                    },
                ),
            })
        };
        let role = msg.ck.role.as_str();
        if role == "assistant" {
            builder.state.pending_calls.clear();
            builder.state.call_arcs.clear();
            let mut call_counts = BTreeMap::<&str, usize>::new();
            for block in msg.ck.content() {
                if let BlockKind::ToolCall { id, .. } = block.kind() {
                    *call_counts.entry(id.as_str()).or_default() += 1;
                }
            }
            for (index, block) in msg.ck.content().iter().enumerate() {
                if let BlockKind::ToolCall { id, .. } = block.kind() {
                    let block_id = block_id(&msg.mid, index);
                    let arc_id = if call_counts.get(id.as_str()).copied().unwrap_or(0) > 1 {
                        tool_arc_id(&msg.mid, id)
                    } else {
                        block_id.clone()
                    };
                    builder.state.call_arcs.insert(block_id, arc_id.clone());
                    builder
                        .state
                        .pending_calls
                        .entry(id.clone())
                        .or_default()
                        .push_back(arc_id);
                }
            }
        }

        let mut identities = Vec::new();
        for index in 0..msg.ck.content().len() {
            let arc_id = arc_for_block(
                &msg.mid,
                index,
                &msg.ck,
                &mut builder.state.pending_calls,
                &builder.state.call_arcs,
            )?;
            let flat = flatten_block(&msg, index, arc_id)?;
            if !flat.synthetic {
                identities.push(BlockIdentity {
                    kind_tag: flat.kind_tag.clone(),
                    byte_fingerprint: fingerprint_digest(&flat.content_hash),
                });
            }
            builder.blocks.push(flat);
        }

        // A tool arc ends at the next non-tool-carrying turn only after that message consumes pending calls.
        // The message's blocks must consume pending calls before the tool arc is cleared.
        // On the Anthropic wire, a `tool_result` can share a `USER` message with user text.
        // Ingress errors occur before state commit.
        if role != "assistant" && role != "tool" {
            builder.state.pending_calls.clear();
        }
        if !synthetic {
            builder.identity_by_mid.insert(msg.mid.clone(), identities);
        }
        builder.messages.0.push(msg);
        builder.message_block_ends.push(builder.blocks.len());
        builder
            .states_after_messages
            .push(Arc::new(builder.state.clone()));
    }

    Ok(FlatProjection {
        blocks: builder.blocks,
        identity_by_mid: builder.identity_by_mid,
        messages: builder.messages,
        message_block_ends: builder.message_block_ends,
        states_after_messages: builder.states_after_messages,
    })
}

pub fn block_id(mid: &str, index: usize) -> String {
    format!("{mid}#{index}")
}

pub fn split_block_id(id: &str) -> Option<(&str, usize)> {
    let (mid, index) = id.rsplit_once('#')?;
    if mid.is_empty() || mid.contains('#') {
        return None;
    }
    let index = index.parse().ok()?;
    Some((mid, index))
}

/// Preserves tool identity and provider extras while replacing reducible content.
///
/// A tool result keeps its success, error, or denial classification. Failure state
/// lives in the `OutputKind` variant, not in a sibling flag; collapsing error outputs
/// to `Text` would tell the model a failed call succeeded.
pub fn reduced_block(block: &WireBlock, reduced: &str, file_path: Option<&str>) -> WireBlock {
    let kind = match block.kind() {
        BlockKind::ToolResult {
            id,
            tool_name,
            output,
            provider_executed,
        } => {
            let kind = match &output.kind {
                OutputKind::Text { .. } | OutputKind::Json { .. } | OutputKind::Content { .. } => {
                    OutputKind::Text {
                        text: reduced.to_string(),
                    }
                }
                OutputKind::ErrorText { .. }
                | OutputKind::ErrorJson { .. }
                | OutputKind::ErrorContent { .. } => OutputKind::ErrorText {
                    text: reduced.to_string(),
                },
                OutputKind::ExecutionDenied { .. } => OutputKind::ExecutionDenied {
                    reason: Some(reduced.to_string()),
                },
            };
            BlockKind::ToolResult {
                id: id.clone(),
                tool_name: tool_name.clone(),
                output: ToolOutput {
                    kind,
                    provider_extras: output.provider_extras.clone(),
                },
                provider_executed: *provider_executed,
            }
        }
        BlockKind::ToolCall {
            id,
            name,
            provider_executed,
            ..
        } => {
            let mut input = serde_json::Map::new();
            input.insert("reduced".to_string(), Value::Bool(true));
            input.insert("summary".to_string(), Value::String(reduced.to_string()));
            if let Some(path) = file_path {
                input.insert("path".to_string(), Value::String(path.to_string()));
            }
            BlockKind::ToolCall {
                id: id.clone(),
                name: name.clone(),
                input: Value::Object(input),
                provider_executed: *provider_executed,
            }
        }
        BlockKind::Reasoning { .. } => BlockKind::Reasoning {
            text: reduced.to_string(),
            signature: None,
        },
        BlockKind::Text { .. } | BlockKind::RedactedReasoning { .. } => BlockKind::Text {
            text: reduced.to_string(),
        },
        BlockKind::Media(_) | BlockKind::Opaque(_) => BlockKind::Text {
            text: reduced.to_string(),
        },
    };
    WireBlock::with_provider_extras(kind, block.provider_extras.clone())
}

pub fn text_from_message(msg: &WireMessage) -> Option<&str> {
    match msg.content().first()?.kind() {
        BlockKind::Text { text } => Some(text.as_str()),
        _ => None,
    }
}

fn flatten_block(
    msg: &Arc<IngressMessage>,
    index: usize,
    arc_id: Option<String>,
) -> Result<FlatBlock, WireError> {
    let block = &msg.ck.content()[index];
    let bytes = serde_json::to_string(block).map_err(|_| WireError::UnsupportedBlock {
        mid: msg.mid.clone(),
        block_index: index,
        kind: block.kind().tag().to_string(),
    })?;
    let content_hash: [u8; 32] = Sha256::digest(bytes.as_bytes()).into();
    let (name, file_path, provider_executed, tool_call_id, output_kind) = match block.kind() {
        BlockKind::ToolCall {
            id,
            name,
            input,
            provider_executed,
        } => (
            Some(name.clone()),
            extract_file_path(input),
            *provider_executed,
            Some(id.clone()),
            None,
        ),
        BlockKind::ToolResult {
            id,
            output,
            provider_executed,
            ..
        } => (
            None,
            None,
            *provider_executed,
            Some(id.clone()),
            Some(output.kind.tag().to_string()),
        ),
        _ => (None, None, false, None, None),
    };

    Ok(FlatBlock {
        id: block_id(&msg.mid, index),
        mid: msg.mid.clone(),
        block_index: index,
        ordinal: msg.ordinal,
        role: msg.ck.role.clone(),
        kind_tag: block.kind().tag().to_string(),
        name,
        file_path,
        provider_executed,
        arc_id,
        bytes: Arc::from(bytes),
        content_hash,
        synthetic: msg.ck.meta.synthetic,
        tool_call_id,
        output_kind,
        wire: SharedWireBlock {
            message: Arc::clone(msg),
            index,
        },
    })
}

fn tool_arc_id(mid: &str, call_id: &str) -> String {
    format!("{mid}#call:{call_id}")
}

fn arc_for_block(
    mid: &str,
    index: usize,
    msg: &WireMessage,
    pending_calls: &mut BTreeMap<String, VecDeque<String>>,
    call_arcs: &BTreeMap<String, String>,
) -> Result<Option<String>, WireError> {
    match msg.content()[index].kind() {
        BlockKind::ToolCall { .. } if msg.role == "assistant" => {
            Ok(call_arcs.get(&block_id(mid, index)).cloned())
        }
        BlockKind::ToolResult { id, .. } => {
            let Some(queue) = pending_calls.get_mut(id) else {
                return Err(WireError::UnpairedToolResult {
                    mid: mid.to_string(),
                    block_index: index,
                    tool_call_id: id.clone(),
                });
            };
            let Some(call_block_id) = queue.pop_front() else {
                return Err(WireError::UnpairedToolResult {
                    mid: mid.to_string(),
                    block_index: index,
                    tool_call_id: id.clone(),
                });
            };
            Ok(Some(call_block_id))
        }
        BlockKind::Reasoning { .. } | BlockKind::RedactedReasoning { .. }
            if msg.role == "assistant" =>
        {
            Ok(adjacent_tool_call_arc(mid, index, msg.content(), call_arcs))
        }
        _ => Ok(None),
    }
}

/// A neighbouring call's arc is read from `call_arcs`, not rebuilt from its block id,
/// because a call whose id repeats within the message carries the shared
/// `mid#call:<id>` arc rather than `mid#index`.
fn adjacent_tool_call_arc(
    mid: &str,
    index: usize,
    content: &[WireBlock],
    call_arcs: &BTreeMap<String, String>,
) -> Option<String> {
    let neighbour =
        |neighbour_index: usize| call_arcs.get(&block_id(mid, neighbour_index)).cloned();
    if index > 0 && matches!(content[index - 1].kind(), BlockKind::ToolCall { .. }) {
        return neighbour(index - 1);
    }
    if index + 1 < content.len() && matches!(content[index + 1].kind(), BlockKind::ToolCall { .. })
    {
        return neighbour(index + 1);
    }
    None
}

fn extract_file_path(input: &Value) -> Option<String> {
    let obj = input.as_object()?;
    ["filePath", "file_path", "path"]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str).map(str::to_string))
}

pub(crate) fn fingerprint_digest(content_hash: &[u8; 32]) -> String {
    let mut out = String::with_capacity(content_hash.len() * 2);
    for byte in content_hash {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub(crate) fn fingerprint(bytes: &str) -> String {
    let content_hash: [u8; 32] = Sha256::digest(bytes.as_bytes()).into();
    fingerprint_digest(&content_hash)
}

/// Reuses a positional receipt only when the projected and served blocks are structurally equal.
pub(crate) fn fingerprint_from_projected_wire(
    served: &WireBlock,
    projected: Option<&FlatBlock>,
) -> Option<(String, usize)> {
    let flat = projected?;
    if flat.wire.as_ref() != served {
        return None;
    }
    Some((fingerprint_digest(&flat.content_hash), flat.bytes.len()))
}

/// Hashes block equality identity for request-local fallback candidate selection, not served
/// bytes. The field list mirrors `WireBlock`'s derived `PartialEq` by hand, so callers
/// re-check equality on the selected candidate.
pub(crate) fn block_identity_digest(block: &WireBlock) -> [u8; 32] {
    use serde_json::ser::{CompactFormatter, Formatter};
    use std::io::{self, Write};

    struct IdentityFormatter;
    impl Formatter for IdentityFormatter {
        fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
            // Normalize -0.0 to 0.0 because `f64` equality treats both values as equal.
            CompactFormatter.write_f64(writer, if value == 0.0 { 0.0 } else { value })
        }
    }

    let mut writer = Sha256::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut writer, IdentityFormatter);
    (block.kind(), &block.provider_extras, block.original())
        .serialize(&mut serializer)
        .expect("CK wire block identity must serialize");
    writer.finalize().into()
}

pub fn duplicate_ids(blocks: &[FlatBlock]) -> Option<String> {
    let mut seen = BTreeSet::new();
    for block in blocks {
        if !seen.insert(block.id.as_str()) {
            return Some(block.id.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_msg(mid: &str, ordinal: u64, role: &str, text: &str) -> Arc<IngressMessage> {
        Arc::new(IngressMessage {
            mid: mid.to_string(),
            ordinal,
            ck: WireMessage::from_parts(
                role,
                vec![WireBlock::bare(BlockKind::Text { text: text.into() })],
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        })
    }

    fn assistant_with_call(mid: &str, ordinal: u64, call_id: &str) -> Arc<IngressMessage> {
        Arc::new(IngressMessage {
            mid: mid.to_string(),
            ordinal,
            ck: WireMessage::from_parts(
                "assistant",
                vec![
                    WireBlock::bare(BlockKind::Text {
                        text: "running a tool".into(),
                    }),
                    WireBlock::bare(BlockKind::ToolCall {
                        id: call_id.to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({}),
                        provider_executed: false,
                    }),
                ],
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        })
    }

    #[test]
    fn projection_retained_bytes_counts_wire_and_frontier_allocations_once() {
        use std::mem::size_of;

        fn manual_value_retained_bytes(value: &Value) -> usize {
            fn heap(value: &Value) -> usize {
                match value {
                    Value::Null | Value::Bool(_) | Value::Number(_) => 0,
                    Value::String(value) => value.capacity(),
                    Value::Array(values) => values
                        .capacity()
                        .saturating_mul(size_of::<Value>())
                        .saturating_add(values.iter().map(heap).sum::<usize>()),
                    Value::Object(values) => values
                        .len()
                        .saturating_mul(
                            size_of::<String>() + size_of::<Value>() + size_of::<usize>() * 3,
                        )
                        .saturating_add(
                            values
                                .iter()
                                .map(|(key, value)| key.capacity().saturating_add(heap(value)))
                                .sum::<usize>(),
                        ),
                }
            }
            size_of::<Value>().saturating_add(heap(value))
        }

        let input = Value::Object(
            (0..96)
                .map(|index| {
                    (
                        format!("k{index}"),
                        serde_json::json!({"v": format!("x{index}"), "ok": index % 2 == 0}),
                    )
                })
                .collect(),
        );
        let constructed = Arc::new(IngressMessage {
            mid: "tool-heavy".to_string(),
            ordinal: 1,
            ck: WireMessage::from_parts(
                "assistant",
                vec![WireBlock::bare(BlockKind::ToolCall {
                    id: "call-heavy".to_string(),
                    name: "fixture_tool".to_string(),
                    input,
                    provider_executed: false,
                })],
                Some(MessageOrigin {
                    provider: "fixture-provider".into(),
                    model: "fixture-model".into(),
                    api: "chat".into(),
                }),
                serde_json::from_value(serde_json::json!({
                    "fixture-provider": {"cache_control": {"type": "ephemeral"}}
                }))
                .unwrap(),
                HarnessMeta {
                    harness_id: Some("harness-tool-heavy".into()),
                    ordinal: Some(1),
                    summary: true,
                    errored: true,
                    finish: Some("tool_calls".into()),
                    created_at_ms: Some(123_000),
                    synthetic: false,
                },
            ),
        });
        // Reparsing through the wire gives both `WireMessage` and `WireBlock` ownership of the original JSON.
        let message: Arc<IngressMessage> =
            serde_json::from_value(serde_json::to_value(constructed).unwrap()).unwrap();
        let projection = project_messages(&[message]).unwrap();
        let block = &projection.blocks[0];
        let wire_json = serde_json::to_value(block.wire.as_ref()).unwrap();
        let BlockKind::ToolCall {
            id, name, input, ..
        } = block.wire.kind()
        else {
            panic!("fixture must project a tool call");
        };
        let wire_retained = size_of::<WireBlock>()
            .saturating_add(id.capacity())
            .saturating_add(name.capacity())
            .saturating_add(manual_value_retained_bytes(input).saturating_sub(size_of::<Value>()))
            .saturating_add(
                manual_value_retained_bytes(&wire_json).saturating_sub(size_of::<Value>()),
            );
        let block_heap = block
            .id
            .capacity()
            .saturating_add(block.mid.capacity())
            .saturating_add(block.role.capacity())
            .saturating_add(block.kind_tag.capacity())
            .saturating_add(block.name.as_ref().map_or(0, String::capacity))
            .saturating_add(block.file_path.as_ref().map_or(0, String::capacity))
            .saturating_add(block.arc_id.as_ref().map_or(0, String::capacity))
            .saturating_add(block.tool_call_id.as_ref().map_or(0, String::capacity))
            .saturating_add(block.output_kind.as_ref().map_or(0, String::capacity))
            .saturating_add(crate::retained_size::ARC_ALLOCATION_OVERHEAD_BYTES)
            .saturating_add(block.bytes.len());
        let blocks = projection
            .blocks
            .capacity()
            .saturating_mul(size_of::<FlatBlock>())
            .saturating_add(block_heap);
        let identities = projection
            .identity_by_mid
            .len()
            .saturating_mul(
                size_of::<String>() + size_of::<Vec<BlockIdentity>>() + size_of::<usize>() * 3,
            )
            .saturating_add(
                projection
                    .identity_by_mid
                    .iter()
                    .map(|(mid, identities)| {
                        mid.capacity()
                            .saturating_add(
                                identities
                                    .capacity()
                                    .saturating_mul(size_of::<BlockIdentity>()),
                            )
                            .saturating_add(
                                identities
                                    .iter()
                                    .map(|identity| {
                                        identity
                                            .kind_tag
                                            .capacity()
                                            .saturating_add(identity.byte_fingerprint.capacity())
                                    })
                                    .sum::<usize>(),
                            )
                    })
                    .sum::<usize>(),
            );
        let frontiers = projection
            .states_after_messages
            .capacity()
            .saturating_mul(size_of::<Arc<ProjectionState>>())
            .saturating_add(
                projection
                    .states_after_messages
                    .iter()
                    .map(|state| {
                        let pending = state
                            .pending_calls
                            .len()
                            .saturating_mul(
                                size_of::<String>()
                                    + size_of::<VecDeque<String>>()
                                    + size_of::<usize>() * 3,
                            )
                            .saturating_add(
                                state
                                    .pending_calls
                                    .iter()
                                    .map(|(key, values)| {
                                        key.capacity()
                                            .saturating_add(
                                                values
                                                    .capacity()
                                                    .saturating_mul(size_of::<String>()),
                                            )
                                            .saturating_add(
                                                values.iter().map(String::capacity).sum::<usize>(),
                                            )
                                    })
                                    .sum::<usize>(),
                            );
                        let arcs = state
                            .call_arcs
                            .len()
                            .saturating_mul(size_of::<String>() * 2 + size_of::<usize>() * 3)
                            .saturating_add(
                                state
                                    .call_arcs
                                    .iter()
                                    .map(|(key, value)| {
                                        key.capacity().saturating_add(value.capacity())
                                    })
                                    .sum::<usize>(),
                            );
                        crate::retained_size::ARC_ALLOCATION_OVERHEAD_BYTES
                            .saturating_add(size_of::<ProjectionState>())
                            .saturating_add(pending)
                            .saturating_add(arcs)
                    })
                    .sum::<usize>(),
            );
        let shell = &projection.messages[0].ck;
        let origin = shell.origin.as_ref().unwrap();
        let origin_heap =
            origin.provider.capacity() + origin.model.capacity() + origin.api.capacity();
        let provider_heap = shell.provider_extras.len()
            * (size_of::<String>() + size_of::<BTreeMap<String, Value>>() + size_of::<usize>() * 3)
            + shell
                .provider_extras
                .iter()
                .map(|(namespace, fields)| {
                    namespace.capacity()
                        + fields.len()
                            * (size_of::<String>() + size_of::<Value>() + size_of::<usize>() * 3)
                        + fields
                            .iter()
                            .map(|(key, value)| {
                                key.capacity() + manual_value_retained_bytes(value)
                                    - size_of::<Value>()
                            })
                            .sum::<usize>()
                })
                .sum::<usize>();
        let meta_heap = shell.meta.harness_id.as_ref().unwrap().capacity()
            + shell.meta.finish.as_ref().unwrap().capacity();
        assert!(origin_heap > 0 && provider_heap > 0 && meta_heap > 0);
        let expected = size_of::<FlatProjection>()
            .saturating_add(blocks)
            .saturating_add(identities)
            .saturating_add(frontiers)
            .saturating_add(projection.messages.capacity() * size_of::<Arc<IngressMessage>>())
            .saturating_add(size_of::<usize>() * 2)
            .saturating_add(size_of::<IngressMessage>())
            .saturating_add(projection.messages[0].mid.capacity())
            .saturating_add(projection.messages[0].ck.role.capacity())
            .saturating_add(projection.messages[0].ck.content().capacity() * size_of::<WireBlock>())
            .saturating_add(wire_retained - size_of::<WireBlock>())
            .saturating_add(origin_heap)
            .saturating_add(provider_heap)
            .saturating_add(meta_heap)
            .saturating_add(
                projection
                    .message_block_ends
                    .capacity()
                    .saturating_mul(size_of::<usize>()),
            );
        let legacy = projection
            .blocks
            .iter()
            .map(|block| {
                size_of::<FlatBlock>()
                    + block.id.len()
                    + block.mid.len()
                    + block.role.len()
                    + block.kind_tag.len()
                    + block.name.as_deref().map_or(0, str::len)
                    + block.file_path.as_deref().map_or(0, str::len)
                    + block.arc_id.as_deref().map_or(0, str::len)
                    + block.tool_call_id.as_deref().map_or(0, str::len)
                    + block.output_kind.as_deref().map_or(0, str::len)
                    + block.bytes.len() * 3
            })
            .sum::<usize>()
            + projection
                .identity_by_mid
                .iter()
                .map(|(mid, identities)| mid.len() + identities.len() * size_of::<BlockIdentity>())
                .sum::<usize>()
            + projection
                .states_after_messages
                .iter()
                .map(|state| {
                    state
                        .pending_calls
                        .iter()
                        .map(|(key, values)| {
                            key.len() + values.iter().map(String::len).sum::<usize>()
                        })
                        .sum::<usize>()
                        + state
                            .call_arcs
                            .iter()
                            .map(|(key, value)| key.len() + value.len())
                            .sum::<usize>()
                })
                .sum::<usize>()
            + projection.message_block_ends.len() * size_of::<usize>();
        let retained = projection.retained_bytes();

        assert!(retained >= legacy.saturating_mul(2));
        assert_eq!(retained, expected);
    }

    #[test]
    fn repeated_call_id_within_owner_message_shares_one_arc_identity() {
        let message = Arc::new(IngressMessage {
            mid: "assistant-1".to_string(),
            ordinal: 1,
            ck: WireMessage::from_parts(
                "assistant",
                vec![
                    WireBlock::bare(BlockKind::ToolCall {
                        id: "duplicate".into(),
                        name: "read".into(),
                        input: serde_json::json!({"path": "one"}),
                        provider_executed: false,
                    }),
                    WireBlock::bare(BlockKind::ToolCall {
                        id: "duplicate".into(),
                        name: "read".into(),
                        input: serde_json::json!({"path": "two"}),
                        provider_executed: false,
                    }),
                ],
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        });
        let projection = project_messages(&[message]).expect("duplicate call ids are projectable");
        assert_eq!(projection.blocks[0].arc_id, projection.blocks[1].arc_id);
        assert_eq!(
            projection.blocks[0].arc_id.as_deref(),
            Some("assistant-1#call:duplicate")
        );
    }

    #[test]
    fn reasoning_joins_the_arc_its_adjacent_call_was_assigned() {
        fn call(id: &str) -> WireBlock {
            WireBlock::bare(BlockKind::ToolCall {
                id: id.into(),
                name: "read".into(),
                input: serde_json::json!({}),
                provider_executed: false,
            })
        }
        fn reasoning(text: &str) -> WireBlock {
            WireBlock::bare(BlockKind::Reasoning {
                text: text.into(),
                signature: None,
            })
        }
        fn arc(projection: &FlatProjection, id: &str) -> Option<String> {
            projection
                .blocks
                .iter()
                .find(|block| block.id() == id)
                .unwrap_or_else(|| panic!("block {id} projected"))
                .arc_id
                .clone()
        }

        // c0 repeats, so both c0 calls share the `#call:` arc; c1 is alone and keeps its block id.
        let message = Arc::new(IngressMessage {
            mid: "m0".into(),
            ordinal: 0,
            ck: WireMessage::from_parts(
                "assistant",
                vec![
                    reasoning("plan"),
                    call("c0"),
                    WireBlock::bare(BlockKind::RedactedReasoning {
                        data: "hidden".into(),
                    }),
                    call("c0"),
                    call("c1"),
                    reasoning("after"),
                    WireBlock::bare(BlockKind::Text {
                        text: "done".into(),
                    }),
                    reasoning("stranded"),
                ],
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        });
        let projection = project_messages(&[message]).unwrap();

        assert_eq!(arc(&projection, "m0#0"), Some("m0#call:c0".into()));
        assert_eq!(arc(&projection, "m0#1"), Some("m0#call:c0".into()));
        assert_eq!(arc(&projection, "m0#2"), Some("m0#call:c0".into()));
        assert_eq!(arc(&projection, "m0#3"), Some("m0#call:c0".into()));
        assert_eq!(arc(&projection, "m0#4"), Some("m0#4".into()));
        assert_eq!(arc(&projection, "m0#5"), Some("m0#4".into()));
        assert_eq!(arc(&projection, "m0#7"), None);

        let call_arcs: std::collections::BTreeSet<_> = projection
            .blocks
            .iter()
            .filter(|block| block.kind_tag == "tool_call")
            .filter_map(|block| block.arc_id.clone())
            .collect();
        for block in projection
            .blocks
            .iter()
            .filter(|block| block.kind_tag == "reasoning" || block.kind_tag == "redacted_reasoning")
        {
            if let Some(arc_id) = &block.arc_id {
                assert!(call_arcs.contains(arc_id), "{} -> {arc_id}", block.id());
            }
        }
    }

    // A `tool_result` may appear in the next user message beside queued user text while a tool runs.
    // A `tool_result` carried by a user message must pair with the prior assistant `ToolCall` despite the carrying role.
    // The projector clears the arc window after walking blocks so user-carried `tool_result`s pair with the preceding assistant `ToolCall`.
    #[test]
    fn user_carried_tool_result_pairs_with_prior_assistant_call() {
        let user_with_result = Arc::new(IngressMessage {
            mid: "m2".to_string(),
            ordinal: 2,
            ck: WireMessage::from_parts(
                "user",
                vec![
                    WireBlock::bare(BlockKind::ToolResult {
                        id: "toolu_1".to_string(),
                        tool_name: "read".to_string(),
                        output: ToolOutput::bare(OutputKind::Text {
                            text: "file contents".into(),
                        }),
                        provider_executed: false,
                    }),
                    WireBlock::bare(BlockKind::Text {
                        text: "queued user question".into(),
                    }),
                ],
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        });
        let messages = vec![
            text_msg("m0", 0, "user", "start"),
            assistant_with_call("m1", 1, "toolu_1"),
            user_with_result,
        ];
        let projection = project_messages(&messages).expect("user-carried result must pair");
        let result_block = projection
            .blocks
            .iter()
            .find(|b| b.id == "m2#0")
            .expect("result block present");
        assert_eq!(
            result_block.arc_id.as_deref(),
            Some("m1#1"),
            "result pairs to the prior assistant's call block"
        );
        // A user message ends the arc window; a later stray result must fail.
        let mut with_stray = messages.clone();
        with_stray.push(Arc::new(IngressMessage {
            mid: "m3".to_string(),
            ordinal: 3,
            ck: WireMessage::from_parts(
                "tool",
                vec![WireBlock::bare(BlockKind::ToolResult {
                    id: "toolu_1".to_string(),
                    tool_name: "read".to_string(),
                    output: ToolOutput::bare(OutputKind::Text {
                        text: "again".into(),
                    }),
                    provider_executed: false,
                })],
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        }));
        let err = project_messages(&with_stray).expect_err("arc window closed by user turn");
        assert!(matches!(err, WireError::UnpairedToolResult { .. }));
    }

    #[test]
    fn user_carried_tool_result_without_prior_call_still_rejects() {
        let messages = vec![
            text_msg("m0", 0, "user", "start"),
            Arc::new(IngressMessage {
                mid: "m1".to_string(),
                ordinal: 1,
                ck: WireMessage::from_parts(
                    "user",
                    vec![WireBlock::bare(BlockKind::ToolResult {
                        id: "toolu_orphan".to_string(),
                        tool_name: "read".to_string(),
                        output: ToolOutput::bare(OutputKind::Text { text: "x".into() }),
                        provider_executed: false,
                    })],
                    None,
                    ProviderExtras::new(),
                    HarnessMeta::default(),
                ),
            }),
        ];
        let err = project_messages(&messages).expect_err("orphan result must reject");
        assert!(matches!(err, WireError::UnpairedToolResult { .. }));
    }

    #[test]
    fn opaque_and_media_inside_tool_result_content_are_accepted_and_projected() {
        let result_with_opaque = Arc::new(IngressMessage {
            mid: "m2".to_string(),
            ordinal: 2,
            ck: WireMessage::from_parts(
                "tool",
                vec![WireBlock::bare(BlockKind::ToolResult {
                    id: "toolu_1".to_string(),
                    tool_name: "computer".to_string(),
                    output: ToolOutput::bare(OutputKind::Content {
                        blocks: vec![
                            ResultBlock {
                                kind: ResultBlockKind::Text {
                                    text: "screenshot captured".into(),
                                },
                                provider_extras: ProviderExtras::new(),
                            },
                            ResultBlock {
                                kind: ResultBlockKind::Opaque {
                                    opaque: OpaqueBlock {
                                        source: serde_json::json!({"source": "wire", "wire": "anthropic"}),
                                        kind: "image".to_string(),
                                        raw: serde_json::json!([1, 2, 3]),
                                        arc: None,
                                    },
                                },
                                provider_extras: ProviderExtras::new(),
                            },
                        ],
                    }),
                    provider_executed: false,
                })],
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        });
        let messages = vec![
            text_msg("m0", 0, "user", "start"),
            assistant_with_call("m1", 1, "toolu_1"),
            result_with_opaque,
        ];
        let projection =
            project_messages(&messages).expect("result-embedded opaque must be accepted");
        assert!(projection.blocks.iter().any(|b| b.id == "m2#0"));

        let mut with_media = messages;
        if let BlockKind::ToolResult { output, .. } =
            Arc::make_mut(&mut with_media[2]).ck.content_mut()[0].kind_mut()
            && let OutputKind::Content { blocks } = &mut output.kind
        {
            blocks[1].kind = ResultBlockKind::Media {
                media: MediaBlock {
                    kind: MediaKind::Image,
                    media_type: "image/png".to_string(),
                    filename: Some("capture.png".to_string()),
                    source: serde_json::json!({"type": "url", "url": "file://capture.png"}),
                },
            };
        }
        let media_projection =
            project_messages(&with_media).expect("result-embedded media must be accepted");
        assert!(
            media_projection
                .blocks
                .iter()
                .find(|block| block.id == "m2#0")
                .expect("media result block")
                .bytes
                .contains("file://capture.png")
        );
    }

    #[test]
    fn incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state() {
        let mut messages = vec![
            text_msg("m0", 0, "user", "start"),
            assistant_with_call("m1", 1, "toolu_1"),
            Arc::new(IngressMessage {
                mid: "m2".to_string(),
                ordinal: 2,
                ck: WireMessage::from_parts(
                    "tool",
                    vec![WireBlock::bare(BlockKind::ToolResult {
                        id: "toolu_1".to_string(),
                        tool_name: "read".to_string(),
                        output: ToolOutput::bare(OutputKind::Text {
                            text: "first result".into(),
                        }),
                        provider_executed: false,
                    })],
                    None,
                    ProviderExtras::new(),
                    HarnessMeta::default(),
                ),
            }),
        ];
        let cached = project_messages(&messages).expect("initial projection");
        let reattached = cached
            .reattach_messages_prefix(2)
            .expect("cached projection rebuilds its acknowledged ingress prefix");
        assert_eq!(reattached[..], messages[..2]);
        if let BlockKind::ToolResult { output, .. } =
            Arc::make_mut(&mut messages[2]).ck.content_mut()[0].kind_mut()
        {
            output.kind = OutputKind::Text {
                text: "changed result".into(),
            };
        }

        let incremental =
            project_messages_incremental(&messages, &cached, 2).expect("incremental projection");
        let full = project_messages(&messages).expect("full projection");
        assert_eq!(incremental, full);
        assert_eq!(incremental.differential_bytes(), full.differential_bytes());
        assert!(std::ptr::eq(
            incremental.blocks[0].wire.as_ref(),
            cached.blocks[0].wire.as_ref()
        ));
        assert!(Arc::ptr_eq(
            &incremental.blocks[0].bytes,
            &cached.blocks[0].bytes
        ));
        assert_eq!(
            incremental.blocks[2].arc_id.as_deref(),
            Some("m1#1"),
            "the cached frontier must carry the pending tool call into the suffix"
        );
    }

    #[test]
    fn empty_and_reserved_message_ids_are_rejected() {
        let empty = vec![text_msg("m0", 0, "user", "a"), text_msg("", 7, "user", "b")];
        assert_eq!(
            project_messages(&empty).unwrap_err(),
            WireError::EmptyMid { ordinal: 7 }
        );
        let reserved = vec![text_msg("bad#id", 2, "user", "c")];
        assert_eq!(
            project_messages(&reserved).unwrap_err(),
            WireError::MidContainsReservedHash("bad#id".into())
        );
        assert_eq!(split_block_id("#3"), None);
    }

    /// Two messages sharing a mid would share every `mid#index` block id, so
    /// one message's blocks would stand for the other's in identity matching.
    #[test]
    fn duplicate_message_ids_are_rejected_across_the_incremental_prefix() {
        let duplicated = vec![
            text_msg("m0", 0, "user", "a"),
            text_msg("m1", 1, "assistant", "b"),
            text_msg("m0", 2, "user", "c"),
        ];
        assert_eq!(
            project_messages(&duplicated).unwrap_err(),
            WireError::DuplicateMid("m0".into())
        );

        // The incremental path carries the cached prefix's mids into the check.
        let prefix = vec![
            text_msg("m0", 0, "user", "a"),
            text_msg("m1", 1, "assistant", "b"),
        ];
        let cached = project_messages(&prefix).unwrap();
        let extended = vec![
            text_msg("m0", 0, "user", "a"),
            text_msg("m1", 1, "assistant", "b"),
            text_msg("m1", 2, "user", "c"),
        ];
        assert_eq!(
            project_messages_incremental(&extended, &cached, 2).unwrap_err(),
            WireError::DuplicateMid("m1".into())
        );
    }

    #[test]
    fn reduced_tool_result_keeps_failure_variant_and_output_extras() {
        let mut output_extras = ProviderExtras::new();
        output_extras
            .entry("anthropic".into())
            .or_default()
            .insert("cache_control".into(), Value::from("ephemeral"));
        let mut block_extras = ProviderExtras::new();
        block_extras
            .entry("openai".into())
            .or_default()
            .insert("item_id".into(), Value::from("it_1"));

        let reduced_text = OutputKind::Text {
            text: "reduced".into(),
        };
        let reduced_error = OutputKind::ErrorText {
            text: "reduced".into(),
        };
        let reduced_denied = OutputKind::ExecutionDenied {
            reason: Some("reduced".into()),
        };
        let cases = [
            (OutputKind::Text { text: "ok".into() }, reduced_text.clone()),
            (
                OutputKind::Json {
                    value: serde_json::json!({"ok": true}),
                },
                reduced_text.clone(),
            ),
            (OutputKind::Content { blocks: vec![] }, reduced_text),
            (
                OutputKind::ErrorText {
                    text: "boom".into(),
                },
                reduced_error.clone(),
            ),
            (
                OutputKind::ErrorJson {
                    value: serde_json::json!({"code": 7}),
                },
                reduced_error.clone(),
            ),
            (OutputKind::ErrorContent { blocks: vec![] }, reduced_error),
            (
                OutputKind::ExecutionDenied {
                    reason: Some("policy".into()),
                },
                reduced_denied.clone(),
            ),
            (OutputKind::ExecutionDenied { reason: None }, reduced_denied),
        ];
        for (original, expected) in cases {
            let block = WireBlock::with_provider_extras(
                BlockKind::ToolResult {
                    id: "c0".into(),
                    tool_name: "bash".into(),
                    output: ToolOutput {
                        kind: original.clone(),
                        provider_extras: output_extras.clone(),
                    },
                    provider_executed: true,
                },
                block_extras.clone(),
            );
            let reduced = reduced_block(&block, "reduced", None);
            assert_eq!(
                *reduced.kind(),
                BlockKind::ToolResult {
                    id: "c0".into(),
                    tool_name: "bash".into(),
                    output: ToolOutput {
                        kind: expected,
                        provider_extras: output_extras.clone(),
                    },
                    provider_executed: true,
                },
                "{}",
                original.tag()
            );
            assert_eq!(reduced.provider_extras, block_extras);
        }
    }

    #[test]
    fn reattach_keeps_block_level_original_but_rebuilds_the_message_shell() {
        let mut json = serde_json::to_value(text_msg("m0", 0, "user", "hello")).unwrap();
        json["ck"]["future_field"] = Value::from(1);
        json["ck"]["content"][0]["future_block_field"] = Value::from(2);
        json["ck"]["origin"] = serde_json::json!({
            "provider": "fixture-provider", "model": "fixture-model", "api": "chat"
        });
        json["ck"]["provider_extras"] = serde_json::json!({
            "fixture-provider": {"cache_control": {"type": "ephemeral"}, "sequence": [1, 2]}
        });
        json["ck"]["meta"] = serde_json::json!({
            "harness_id": "harness-m0", "ordinal": 7, "summary": true,
            "errored": true, "finish": "stop", "created_at_ms": 123_000
        });
        let message: Arc<IngressMessage> = serde_json::from_value(json.clone()).unwrap();
        assert!(!message.ck.meta.synthetic);
        assert_ne!(message.ck.meta, HarnessMeta::default());
        let projection = project_messages(std::slice::from_ref(&message)).unwrap();

        let reattached = projection.reattach_messages_prefix(1).unwrap();
        let replayed = serde_json::to_value(&reattached[0].ck).unwrap();
        assert_eq!(replayed.get("future_field"), None);
        assert_eq!(replayed["content"][0]["future_block_field"], Value::from(2));
        assert_eq!(reattached[0].mid, message.mid);
        assert_eq!(reattached[0].ordinal, message.ordinal);
        assert_eq!(reattached[0].ck.origin, message.ck.origin);
        assert_eq!(reattached[0].ck.provider_extras, message.ck.provider_extras);
        assert_eq!(reattached[0].ck.meta, message.ck.meta);
        assert_eq!(reattached[0].ck.content(), message.ck.content());
        assert!(reattached[0].ck.original().is_none());
        assert!(reattached[0].ck.content()[0].original().is_some());
        let mut expected = json["ck"].clone();
        expected.as_object_mut().unwrap().remove("future_field");
        assert_eq!(
            serde_json::to_vec(&replayed).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        assert_eq!(serde_json::to_value(&message).unwrap(), json);
    }

    #[test]
    fn repeated_prefix_reattachment_shares_canonical_shells() {
        let mut json = serde_json::to_value(text_msg("m0", 0, "user", "prefix")).unwrap();
        json["ck"]["future_field"] = Value::from(1);
        json["ck"]["content"][0]["future_block_field"] = Value::from(2);
        let messages: IngressMessages = serde_json::from_value(serde_json::json!([json])).unwrap();
        let original_bytes = serde_json::to_vec(&messages).unwrap();
        let cached = project_messages(&messages).unwrap();
        let first = cached.reattach_messages_prefix(1).unwrap();
        let second = cached.reattach_messages_prefix(1).unwrap();
        assert!(std::ptr::eq(&first[0].ck, &second[0].ck));
        assert!(std::ptr::eq(
            cached.blocks[0].wire.as_ref(),
            &first[0].ck.content()[0]
        ));
        let projected = project_messages(&first).unwrap();
        let third = projected.reattach_messages_prefix(1).unwrap();
        assert!(std::ptr::eq(&first[0].ck, &third[0].ck));
        let shared = first.clone();
        let incremental = project_messages_incremental(&shared, &cached, 1).unwrap();
        assert!(Arc::ptr_eq(&incremental.messages[0], &first[0]));
        for projection in [&projected, &incremental] {
            assert_eq!(projection, &cached);
            assert_eq!(projection.differential_bytes(), cached.differential_bytes());
            assert_eq!(
                projection.blocks[0].content_hash,
                cached.blocks[0].content_hash
            );
            assert_eq!(projection.identity_by_mid, cached.identity_by_mid);
        }
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&shared).unwrap()
        );
        assert_eq!(serde_json::to_vec(&messages).unwrap(), original_bytes);
        assert!(first[0].ck.original().is_none());
        assert!(first[0].ck.content()[0].original().is_some());
        let mut edited = shared;
        Arc::make_mut(&mut edited[0]).ck.content_mut().clear();
        assert!(!Arc::ptr_eq(&first[0], &edited[0]));
        assert_eq!(first[0].ck.content().len(), 1);
        assert_eq!(cached.reattach_messages_prefix(1).unwrap(), first);
        let block = cached.blocks[0].clone();
        drop(cached);
        assert!(std::ptr::eq(block.wire.as_ref(), &first[0].ck.content()[0]));
    }

    #[test]
    fn shared_ingress_is_send_and_preserves_decode_refusals() {
        fn assert_send<T: Send + 'static>() {}
        assert_send::<IngressMessages>();
        assert_send::<FlatProjection>();
        assert_send::<crate::transform::TransformRequest>();
        for body in [
            "null",
            "{}",
            "[null]",
            r#"[{"mid":7,"ordinal":0,"ck":{}}]"#,
            r#"[{"mid":"m0","ordinal":-1,"ck":{}}]"#,
            r#"[{"mid":"m0","ordinal":0}]"#,
            r#"[{"mid":"m0","mid":"m1","ordinal":0,"ck":{}}]"#,
            "[",
        ] {
            let owned = serde_json::from_str::<Vec<IngressMessage>>(body).unwrap_err();
            let shared = serde_json::from_str::<IngressMessages>(body).unwrap_err();
            assert_eq!(shared.to_string(), owned.to_string(), "{body}");
        }
    }

    #[test]
    fn incremental_projection_checks_effective_synthetic_status() {
        let messages = [
            text_msg("m0", 0, "user", "prefix"),
            text_msg("m1", 1, "user", "tail"),
        ];
        for cached_synthetic in [false, true] {
            let mut prior = MessageProjection::new(&messages);
            if cached_synthetic {
                prior.mark_synthetic(&messages[0]);
            }
            let cached = prior.project().unwrap();
            for synthetic in [false, true] {
                let mut current = MessageProjection::new(&messages);
                if synthetic {
                    current.mark_synthetic(&messages[0]);
                }
                let (incremental, reused_messages) =
                    current.project_incremental(&cached, 1).unwrap();
                assert_eq!(reused_messages, usize::from(cached_synthetic == synthetic));
                assert_eq!(incremental, current.project().unwrap());
                assert_eq!(
                    std::ptr::eq(
                        incremental.blocks[0].wire.as_ref(),
                        cached.blocks[0].wire.as_ref()
                    ),
                    cached_synthetic == synthetic,
                );
                assert_eq!(
                    Arc::ptr_eq(&incremental.blocks[0].bytes, &cached.blocks[0].bytes),
                    cached_synthetic == synthetic,
                );
                assert_eq!(
                    Arc::ptr_eq(&incremental.messages[0], &cached.messages[0]),
                    cached_synthetic == synthetic,
                );
                assert_eq!(
                    incremental.reattach_messages_prefix(1).unwrap()[0]
                        .ck
                        .meta
                        .synthetic,
                    synthetic
                );
            }
        }
    }
}
