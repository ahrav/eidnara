//! Wire-message projection into stable cache block identities.
//!
//! The cache machinery uses stable block identities instead of full wire messages.
//! This module bridges full wire messages and stable block identities.
//! The module assigns each content block a session-stable `mid#block_index` identity.
//! Projections hold `Arc` shells of the ingress messages, so an unreduced response serves them without rebuilding.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;

use memory_store::BlockIdentity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

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

/// Tool-arc state the projector carries from one message to the next.
#[derive(Debug, Default)]
struct ProjectionState {
    pending_calls: BTreeMap<String, VecDeque<String>>,
    call_arcs: BTreeMap<String, String>,
}

/// Flattened message projection.
///
/// Block order follows message order and then content order.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatProjection {
    pub blocks: Vec<FlatBlock>,
    pub identity_by_mid: BTreeMap<String, Vec<BlockIdentity>>,
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
}

#[derive(Default)]
struct FlatProjectionBuilder {
    blocks: Vec<FlatBlock>,
    identity_by_mid: BTreeMap<String, Vec<BlockIdentity>>,
    state: ProjectionState,
}

fn project_messages_from_state(
    ingress: &MessageProjection<'_>,
    mut builder: FlatProjectionBuilder,
) -> Result<FlatProjection, WireError> {
    // Block ids are `mid#index`, so a repeated mid would give two messages'
    // blocks the same identities and let one message's content stand for the
    // other's. Synthetic messages take part: their block ids collide too.
    let mut seen_mids = BTreeSet::new();
    for msg in ingress.messages {
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
        // The shell is shared unless the effective synthetic flag differs.
        let msg = if msg.ck.meta.synthetic == synthetic {
            Arc::clone(msg)
        } else {
            let mut rebuilt = IngressMessage::clone(msg);
            rebuilt.ck.meta.synthetic = synthetic;
            Arc::new(rebuilt)
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
    }

    Ok(FlatProjection {
        blocks: builder.blocks,
        identity_by_mid: builder.identity_by_mid,
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
    let bytes = crate::served_json::canonical_block_bytes(block).map_err(|_| {
        WireError::UnsupportedBlock {
            mid: msg.mid.clone(),
            block_index: index,
            kind: block.kind().tag().to_string(),
        }
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
/// bytes. Signed zeros hash alike because `f64` equality treats them alike; callers re-check
/// equality on the selected candidate.
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
    block
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
    fn duplicate_message_ids_are_rejected() {
        let duplicated = vec![
            text_msg("m0", 0, "user", "a"),
            text_msg("m1", 1, "assistant", "b"),
            text_msg("m0", 2, "user", "c"),
        ];
        assert_eq!(
            project_messages(&duplicated).unwrap_err(),
            WireError::DuplicateMid("m0".into())
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

    /// A message whose effective synthetic flag differs from its ingress flag gets a rebuilt shell with only that flag changed; the ingress shell is untouched.
    #[test]
    fn projection_rebuilds_only_the_shell_whose_synthetic_flag_changes() {
        let pair = crate::injection::build_synthetic_todo_pair(
            r#"[{"content":"rebuild","status":"pending","priority":"high"}]"#,
        )
        .unwrap();
        let mut unflagged = pair.assistant_msg.clone();
        unflagged.meta.synthetic = false;
        let messages = IngressMessages(vec![
            text_msg("m0", 0, "user", "live"),
            Arc::new(IngressMessage {
                mid: "m1".into(),
                ordinal: 1,
                ck: unflagged.clone(),
            }),
        ]);
        let mut ingress = MessageProjection::new(&messages);
        ingress.mark_synthetic(&messages[1]);
        let projection = ingress.project().unwrap();
        let shell = |mid: &str| {
            let block = projection.blocks.iter().find(|block| block.mid == mid);
            Arc::clone(&block.unwrap().wire.message)
        };
        let (live, rebuilt) = (shell("m0"), shell("m1"));
        assert!(Arc::ptr_eq(&live, &messages[0]));
        assert!(!Arc::ptr_eq(&rebuilt, &messages[1]));
        assert!(rebuilt.ck.meta.synthetic);
        assert!(!messages[1].ck.meta.synthetic);
        let mut expected = unflagged;
        expected.meta.synthetic = true;
        assert_eq!(rebuilt.ck, expected);
        assert_eq!(rebuilt.mid, "m1");
        assert_eq!(rebuilt.ordinal, 1);
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
}
