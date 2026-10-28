//! Turns native OpenCode and Pi session records into kernel source descriptors: one occurrence per ordered text block of a message, and one per tool output or error string, each bound to the native identity the harness gave it.
//!
//! Identity is native or refused. A message occurrence names the project, harness, durable session, native message, and block position; a raw tool occurrence names the parent message, tool call, result revision, and output block. A payload hash never stands in for a missing identity. An OpenCode record names its session and is refused when it names another; Pi entries carry no session, so the caller's binding of the session file is the session identity. Message parts that are not text (reasoning, steps, files, patches, images) are not text sources and contribute nothing; a tool result whose content is not text is refused, because the tool string is the source and a non-text one has no disposition here. Tool strings are retained exactly as the codec received them and are never joined; a tool result is never part of a message's text, and its class creates no dense work downstream.
//!
//! Publication retains each block's exact bytes, then commits its descriptor and admission. Receipt keys make replays idempotent; changed bytes or publishing context conflict. A newer revision invalidates its predecessor atomically. Permanent descriptor refusals attempt to retire retained evidence. Kernel failures, including unmet preconditions such as a missing scope, preserve it; callers decide whether the failure permits retry.
//!
//! [`publish_screened`] refuses units larger than [`MAX_SOURCE_UNIT_BYTES`] whole to preserve exact source text. Each batch admits at most [`MAX_SOURCE_BATCH_UNITS`] units, [`MAX_SOURCE_BATCH_BYTES`] of text, and one unit per lineage; excess units wait for a later pass. Publication checks size, identity, revision lead, and scope per unit before the kernel transaction. When the transaction refuses a descriptor, such as a revision that does not advance its lineage, publication removes that unit and retries the remaining units. Each transaction commits its units atomically.

use kernel::source_identity::{
    HARNESSES, Occurrence, OccurrenceClass, encode_preserving_span, identity_digest, lower_hex,
    occurrence_id_preserving_span,
};

const _: () = assert!(
    HARNESSES.len() == 2,
    "both harness spellings are the kernel's"
);
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactErrorKind, ArtifactHandle, ArtifactIngestRequest,
    CommitIntent, EventKind, KernelError, KernelStore, ProjectScope, ProviderEgress, Sensitivity,
    SourceClass, SourceDescriptorError, SourceDescriptorOutcome, SourceDescriptorPolicy,
    SourceDescriptorRequest, TaintClass, ToolOutcome,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Harness {
    OpenCode,
    Pi,
}

impl Harness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    /// The harness whose `as_str` is `value`.
    pub fn parse(value: &str) -> Option<Self> {
        [Self::OpenCode, Self::Pi]
            .into_iter()
            .find(|harness| harness.as_str() == value)
    }
}

/// The route-bound identity every unit of a session is published under. `project_id` is the bound project's digest, the scope term's exact value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionIdentity {
    pub project_id: String,
    pub harness: Harness,
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Representation {
    Text,
    ToolOutput,
    ToolError,
    DecisionSummary,
    Rationale,
    Summary,
    CommitMessage,
}

impl Representation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::ToolOutput => "tool_output",
            Self::ToolError => "tool_error",
            Self::DecisionSummary => "decision_summary",
            Self::Rationale => "rationale",
            Self::Summary => "summary",
            Self::CommitMessage => "commit_message",
        }
    }
}

/// Units with this role take their admission classes from the source decision's own admission at publication instead of a role mapping.
pub const CANONICAL_ROLE: &str = "canonical";

/// The frozen git source policy every `git_commits` descriptor is published under: the exact commit message bytes of one explicitly selected commit, no traversal.
pub const GIT_SOURCE_POLICY_VERSION: &str = "git-commit-message.v1";

/// One publishable occurrence: its class, its complete native identity in the class's field order, its canonical revision, and the exact text of the block.
#[derive(Clone, PartialEq, Eq)]
pub struct SourceUnit {
    pub class: OccurrenceClass,
    pub identity: Vec<(&'static str, String)>,
    pub revision: String,
    pub representation: Representation,
    pub text: String,
    /// The native role of the message the unit came from (`user`, `assistant`, `toolResult`); provenance, never identity or text.
    pub role: String,
    /// How the tool call behind a `raw_tool_spans` unit settled ([`crate::tool_outcome`]); every other class carries `None`.
    pub outcome: Option<ToolOutcome>,
}

/// Debug names identities and sizes, never the text.
impl std::fmt::Debug for SourceUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceUnit")
            .field("class", &self.class.code())
            .field("identity", &self.identity)
            .field("revision", &self.revision)
            .field("representation", &self.representation.as_str())
            .field("text_bytes", &self.text.len())
            .field("role", &self.role)
            .field(
                "outcome",
                &self.outcome.as_ref().map(|outcome| outcome.status),
            )
            .finish()
    }
}

impl SourceUnit {
    pub(crate) fn identity_value(&self, name: &str) -> Option<&str> {
        self.identity
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.text.len()
            + self.role.len()
            + self.revision.len()
            + self
                .identity
                .iter()
                .map(|(_, value)| 48 + value.len())
                .sum::<usize>()
    }

    /// The actor a unit's commits and evidence are recorded under: the harness that produced it, the repository a commit came from, or the canonical role for a unit derived from a kernel decision.
    fn origin(&self) -> &str {
        self.identity_value("harness")
            .or_else(|| self.identity_value("repository_id"))
            .unwrap_or(CANONICAL_ROLE)
    }

    /// The kernel accepts a `git_commits` descriptor only under the git policy and every other class only under the native one.
    fn source_policy(&self) -> SourceDescriptorPolicy {
        match self.class {
            OccurrenceClass::GitCommits => SourceDescriptorPolicy::Git {
                version: GIT_SOURCE_POLICY_VERSION.to_owned(),
            },
            _ => SourceDescriptorPolicy::Native,
        }
    }
}

/// Why a native record produced no units. Every variant names a field or shape, never content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterRefusal {
    #[error("the native record is not an object")]
    NotAnObject,
    #[error("the native record has no {0}")]
    MissingIdentity(&'static str),
    #[error("the native record belongs to another session")]
    SessionMismatch,
    #[error("the native {0} is not a canonical revision")]
    MalformedRevision(&'static str),
    #[error("the native {0} has a shape this adapter has no disposition for")]
    UnsupportedShape(&'static str),
}

fn field<'v>(value: &'v Value, name: &'static str) -> Result<&'v str, AdapterRefusal> {
    nonempty(value.get(name), name)
}

/// The non-empty string `value` holds, or the refusal naming the missing `name`.
fn nonempty<'v>(value: Option<&'v Value>, name: &'static str) -> Result<&'v str, AdapterRefusal> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or(AdapterRefusal::MissingIdentity(name))
}

/// A native millisecond timestamp as the canonical decimal revision string the kernel compares numerically.
fn revision(value: Option<&Value>, name: &'static str) -> Result<String, AdapterRefusal> {
    match value.and_then(Value::as_u64) {
        Some(millis) if i64::try_from(millis).is_ok() => Ok(millis.to_string()),
        _ => Err(AdapterRefusal::MalformedRevision(name)),
    }
}

/// The identity of one `messages` unit in the class's field order. Memory
/// capture rebuilds it from a capture job to read the unit's descriptor.
pub(crate) fn message_identity(
    project_id: &str,
    harness: &str,
    session_id: &str,
    message_id: &str,
    block_index: usize,
) -> Vec<(&'static str, String)> {
    vec![
        ("project_id", project_id.to_owned()),
        ("harness", harness.to_owned()),
        ("session_id", session_id.to_owned()),
        ("message_id", message_id.to_owned()),
        ("block_index", block_index.to_string()),
    ]
}

impl SessionIdentity {
    fn message_unit(
        &self,
        message_id: &str,
        block_index: usize,
        revision: &str,
        role: &str,
        text: &str,
    ) -> SourceUnit {
        SourceUnit {
            class: OccurrenceClass::Messages,
            identity: message_identity(
                &self.project_id,
                self.harness.as_str(),
                &self.session_id,
                message_id,
                block_index,
            ),
            revision: revision.to_owned(),
            representation: Representation::Text,
            text: text.to_owned(),
            role: role.to_owned(),
            outcome: None,
        }
    }

    fn tool_unit(
        &self,
        tool: &ToolBlock<'_>,
        block_index: usize,
        text: &str,
        outcome: ToolOutcome,
    ) -> SourceUnit {
        SourceUnit {
            class: OccurrenceClass::RawToolSpans,
            identity: vec![
                ("project_id", self.project_id.clone()),
                ("harness", self.harness.as_str().to_owned()),
                ("session_id", self.session_id.clone()),
                ("parent_message_id", tool.parent_message_id.to_owned()),
                ("tool_call_id", tool.tool_call_id.to_owned()),
                ("result_revision", tool.result_revision.clone()),
                ("block_index", block_index.to_string()),
            ],
            revision: tool.result_revision.clone(),
            representation: if tool.is_error {
                Representation::ToolError
            } else {
                Representation::ToolOutput
            },
            text: text.to_owned(),
            role: "toolResult".to_owned(),
            outcome: Some(outcome),
        }
    }
}

struct ToolBlock<'a> {
    parent_message_id: &'a str,
    tool_call_id: &'a str,
    result_revision: String,
    is_error: bool,
}

/// Units of one OpenCode `MessageV2` record: user text at `time.created`, assistant text at `time.completed`, and settled tool parts at `state.time.end`. Text block indices are part positions; a tool's output block index is 0, its one exact output or error string. Ignored text, synthetic text, and other part kinds contribute nothing.
///
/// Assistant text without a completion timestamp is deferred. A valid completion timestamp makes non-ignored, non-synthetic text eligible even when `info.error` is present. Settled tools contribute independently, except a call the turn's abort interrupted (`state.metadata.interrupted`), which waits for the completion timestamp so its outcome reads the abort's final error. The adapter uses no separate edit revision for user text or completed assistant text; changing bytes at the same native identity and timestamp conflicts at publication.
///
/// # Errors
///
/// The OpenCode adapter refuses a non-OpenCode binding, non-object `info`, missing identities, roles other than `user` or `assistant`, a different session, missing or invalid timestamps, non-array `parts`, non-string selected text, tool parts without `state`, and settled tools without `callID`, `state.time.end`, string output, or a string `tool` name (`UnsupportedShape("tool")`).
pub fn opencode_units(
    session: &SessionIdentity,
    message: &Value,
) -> Result<Vec<SourceUnit>, AdapterRefusal> {
    opencode_units_within(session, message, &mut |_| true)
        .map(|units| units.expect("an allowance that admits every unit never stops"))
}

/// `opencode_units_within` follows [`opencode_units`]'s rules and calls `admit` with each unit's retained byte count as the unit is built. If `admit` returns `false`, the function drops that unit and returns `Ok(None)`.
///
/// # Errors
///
/// As [`opencode_units`].
pub fn opencode_units_within(
    session: &SessionIdentity,
    message: &Value,
    admit: &mut dyn FnMut(usize) -> bool,
) -> Result<Option<Vec<SourceUnit>>, AdapterRefusal> {
    if session.harness != Harness::OpenCode {
        return Err(AdapterRefusal::UnsupportedShape("harness"));
    }
    let info = message
        .get("info")
        .filter(|info| info.is_object())
        .ok_or(AdapterRefusal::NotAnObject)?;
    let message_id = field(info, "id")?;
    if field(info, "sessionID")? != session.session_id {
        return Err(AdapterRefusal::SessionMismatch);
    }
    let role = field(info, "role")?;
    if !matches!(role, "user" | "assistant") {
        return Err(AdapterRefusal::UnsupportedShape("info.role"));
    }
    let time = info
        .get("time")
        .ok_or(AdapterRefusal::MissingIdentity("time"))?;
    let completed = time
        .get("completed")
        .filter(|completed| !completed.is_null());
    let message_revision = revision(completed.or_else(|| time.get("created")), "time")?;
    let parts = message
        .get("parts")
        .and_then(Value::as_array)
        .ok_or(AdapterRefusal::UnsupportedShape("parts"))?;
    let mut kept = Kept::new(admit);
    for (index, part) in parts.iter().enumerate() {
        match part.get("type").and_then(Value::as_str) {
            // Conversation text is a text part with `ignored` and `synthetic`
            // both absent or false; a synthetic part is injected context.
            Some("text")
                if part.get("ignored").and_then(Value::as_bool) == Some(true)
                    || part.get("synthetic").and_then(Value::as_bool) == Some(true) => {}
            Some("text") if role == "assistant" && completed.is_none() => {}
            Some("text") => {
                let text = part
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or(AdapterRefusal::UnsupportedShape("text part"))?;
                if !kept.keep(session.message_unit(
                    message_id,
                    index,
                    &message_revision,
                    role,
                    text,
                )) {
                    return Ok(None);
                }
            }
            Some("tool") => {
                let state = part
                    .get("state")
                    .ok_or(AdapterRefusal::UnsupportedShape("tool part"))?;
                let is_error = match state.get("status").and_then(Value::as_str) {
                    Some("completed") => false,
                    Some("error") => true,
                    _ => continue,
                };
                // A call the turn's abort interrupted is read once the turn completes, when the abort's named error is final; its outcome names that error.
                if crate::tool_outcome::opencode_interrupted(state) && completed.is_none() {
                    continue;
                }
                let output = state
                    .get(if is_error { "error" } else { "output" })
                    .and_then(Value::as_str)
                    .ok_or(AdapterRefusal::UnsupportedShape("tool output"))?;
                let tool = ToolBlock {
                    parent_message_id: message_id,
                    tool_call_id: field(part, "callID")?,
                    result_revision: revision(
                        state.get("time").and_then(|time| time.get("end")),
                        "state.time.end",
                    )?,
                    is_error,
                };
                let outcome = crate::tool_outcome::opencode(part, info.get("error"), output)
                    .ok_or(AdapterRefusal::UnsupportedShape("tool"))?;
                // One settled tool part carries one output string: its only output block.
                if !kept.keep(session.tool_unit(&tool, 0, output, outcome)) {
                    return Ok(None);
                }
            }
            _ => {}
        }
    }
    Ok(Some(kept.units))
}

/// Units of one Pi transform row, which carries its session entry's `type` and `parentId` beside its `id` and `message`: for a user or assistant message, every text content item in position order (a bare string is one block); for a tool result, every text content item as its own block under the call it answers. `tool_call_id` is the entry's `toolCallId` verbatim, including a provider's composite `call|item` spelling; the wire codec's canonical call id is a different value and is not an identity here.
///
/// # Errors
///
/// The Pi adapter refuses a non-Pi binding, non-object entries or messages, a row with a missing, null, or empty `type` (`MissingIdentity("type")`) or whose `type` is not `"message"` (`UnsupportedShape("type")`), missing identities or timestamps, invalid timestamps, unsupported roles, malformed content containers or text items, and tool results without a call, parent, boolean `isError`, or string `toolName` (`UnsupportedShape("message.toolName")`), or with non-text content.
pub fn pi_units(
    session: &SessionIdentity,
    entry: &Value,
) -> Result<Vec<SourceUnit>, AdapterRefusal> {
    pi_units_within(session, entry, &mut |_| true)
        .map(|units| units.expect("an allowance that admits every unit never stops"))
}

/// `pi_units_within` follows [`pi_units`]'s rules and calls `admit` with each unit's retained byte count as the unit is built. If `admit` returns `false`, the function drops that unit and returns `Ok(None)`.
///
/// # Errors
///
/// As [`pi_units`].
pub fn pi_units_within(
    session: &SessionIdentity,
    entry: &Value,
    admit: &mut dyn FnMut(usize) -> bool,
) -> Result<Option<Vec<SourceUnit>>, AdapterRefusal> {
    if session.harness != Harness::Pi {
        return Err(AdapterRefusal::UnsupportedShape("harness"));
    }
    if !entry.is_object() {
        return Err(AdapterRefusal::NotAnObject);
    }
    // The row carries its session entry's type; only a message entry holds text or a tool result.
    match entry.get("type") {
        None | Some(Value::Null) => return Err(AdapterRefusal::MissingIdentity("type")),
        Some(Value::String(kind)) if kind.is_empty() => {
            return Err(AdapterRefusal::MissingIdentity("type"));
        }
        Some(Value::String(kind)) if kind == "message" => {}
        Some(_) => return Err(AdapterRefusal::UnsupportedShape("type")),
    }
    let entry_id = field(entry, "id")?;
    let message_value = entry
        .get("message")
        .ok_or(AdapterRefusal::MissingIdentity("message"))?;
    let message = PiMessage::read(
        message_value
            .as_object()
            .ok_or(AdapterRefusal::MissingIdentity("message"))?,
    );
    let role = nonempty(message.role, "role")?;
    if !matches!(role, "user" | "assistant" | "toolResult") {
        return Err(AdapterRefusal::UnsupportedShape("message.role"));
    }
    let stamp = revision(message.timestamp, "timestamp")?;
    let mut kept = Kept::new(admit);
    if role == "toolResult" {
        let tool = ToolBlock {
            parent_message_id: field(entry, "parentId")?,
            tool_call_id: nonempty(message.tool_call_id, "toolCallId")?,
            result_revision: stamp,
            is_error: message
                .is_error
                .and_then(Value::as_bool)
                .ok_or(AdapterRefusal::UnsupportedShape("message.isError"))?,
        };
        for (index, item) in message.content_items()?.iter().enumerate() {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .filter(|_| item.get("type").and_then(Value::as_str) == Some("text"))
                .ok_or(AdapterRefusal::UnsupportedShape("tool result item"))?;
            let outcome = crate::tool_outcome::pi(message_value, text)
                .ok_or(AdapterRefusal::UnsupportedShape("message.toolName"))?;
            if !kept.keep(session.tool_unit(&tool, index, text, outcome)) {
                return Ok(None);
            }
        }
        return Ok(Some(kept.units));
    }
    match message.content {
        Some(Value::String(text)) => {
            if !kept.keep(session.message_unit(entry_id, 0, &stamp, role, text)) {
                return Ok(None);
            }
        }
        _ => {
            for (index, item) in message.content_items()?.iter().enumerate() {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    let text = item
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or(AdapterRefusal::UnsupportedShape("text item"))?;
                    if !kept.keep(session.message_unit(entry_id, index, &stamp, role, text)) {
                        return Ok(None);
                    }
                }
            }
        }
    }
    Ok(Some(kept.units))
}

struct Kept<'a> {
    units: Vec<SourceUnit>,
    admit: &'a mut dyn FnMut(usize) -> bool,
}

impl<'a> Kept<'a> {
    fn new(admit: &'a mut dyn FnMut(usize) -> bool) -> Self {
        Self {
            units: Vec::new(),
            admit,
        }
    }

    fn keep(&mut self, unit: SourceUnit) -> bool {
        let admitted = (self.admit)(unit.retained_bytes());
        if admitted {
            self.units.push(unit);
        }
        admitted
    }
}

/// The members of a Pi message the adapter reads, collected in one pass over the message object.
#[derive(Default)]
struct PiMessage<'v> {
    role: Option<&'v Value>,
    timestamp: Option<&'v Value>,
    content: Option<&'v Value>,
    tool_call_id: Option<&'v Value>,
    is_error: Option<&'v Value>,
}

impl<'v> PiMessage<'v> {
    fn read(message: &'v serde_json::Map<String, Value>) -> Self {
        let mut read = Self::default();
        for (key, value) in message {
            let slot = match key.as_str() {
                "role" => &mut read.role,
                "timestamp" => &mut read.timestamp,
                "content" => &mut read.content,
                "toolCallId" => &mut read.tool_call_id,
                "isError" => &mut read.is_error,
                _ => continue,
            };
            *slot = Some(value);
        }
        read
    }

    fn content_items(&self) -> Result<&'v Vec<Value>, AdapterRefusal> {
        self.content
            .and_then(Value::as_array)
            .ok_or(AdapterRefusal::UnsupportedShape("content"))
    }
}

/// The receipt outcome of publishing one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub object_id: String,
    pub occurrence_id: String,
    /// The evidence object retaining the unit's exact bytes.
    pub evidence_object_id: String,
    /// The id of that object's evidence row, which a dispute cites.
    pub evidence_id: String,
    /// The predecessor revision this publication invalidated in the same commit.
    pub replaced_object_id: Option<String>,
    /// The kernel answered from a stored receipt: the same unit was published before.
    pub replayed: bool,
    /// The admission classes the unit was recorded under.
    pub admission: (SourceClass, TaintClass),
}

/// Evidence retained before publication failed. Only permanent descriptor refusals attempt compensation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedEvidence {
    pub object_id: String,
    /// `None` means retirement was not attempted. `Some(Ok(()))` confirms retirement;
    /// `Some(Err(error))` reports a failed compensation attempt.
    pub retired: Option<Result<(), KernelError>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The exact artifact was refused: the bytes were rewritten by the scanner, could not be scanned, or exceed the store's bounds. Nothing new was retained.
    #[error("the block's bytes were refused: {0:?}")]
    Artifact(ArtifactErrorKind),
    /// The identity and revision conflict with stored bytes, domain, scope, role, sensitivity, or provider egress policy. Nothing is retained by this request.
    #[error("the identity and revision were already published under another request")]
    IdentityReused,
    /// The decision a canonical unit derives from has no admission decision, so there are no classes to record the descriptor under. `evidence` is `None` when the refusal came before retention.
    #[error("the canonical source has no admission decision")]
    UnadmittedSource { evidence: Option<RetainedEvidence> },
    /// The native revision leads `observed_at` by more than [`MAX_REVISION_LEAD_MS`]. Publishing it would pin the lineage at a revision no later native record could advance, so it is refused before any byte is retained.
    #[error("the native revision {revision} leads the observation at {observed_at}")]
    RevisionAhead { revision: i64, observed_at: i64 },
    /// The descriptor was refused: a malformed or oversized identity before any byte was retained (`evidence` is `None`), or a stale revision, a colliding tuple, a mismatched domain, or a repository document's lineage head that differs from the head the proposal expected. A stale revision refuses the same unit on every attempt, so the producer reads the head and proposes a new unit. A proposal past the head plus one preserves its evidence on `HeadMismatch`, so the same unit can be offered again when the lineage reaches its expected head.
    #[error("the descriptor was refused: {refusal}")]
    Descriptor {
        refusal: SourceDescriptorError,
        evidence: Option<RetainedEvidence>,
    },
    /// A kernel failure, including an unmet precondition such as a missing scope. When present, `evidence` is preserved without attempting retirement. The failure does not imply retryability.
    #[error("{error}")]
    Kernel {
        error: KernelError,
        evidence: Option<RetainedEvidence>,
    },
}

impl PublishError {
    /// Whether the refusal is decided by the unit and the store's identity records alone, so retrying the same unit returns it again. Capacity, storage, kernel, missing-admission, revision-lead, and head-mismatch refusals depend on state that can change and report `false`.
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Artifact(kind) => matches!(
                kind,
                ArtifactErrorKind::PayloadTooLarge
                    | ArtifactErrorKind::ReAdmissionBlocked
                    | ArtifactErrorKind::UnredactableSecret
                    | ArtifactErrorKind::ScanIncomplete
                    | ArtifactErrorKind::DetectionLimit
                    | ArtifactErrorKind::TextFieldTooLong
                    | ArtifactErrorKind::InvalidInput
                    | ArtifactErrorKind::ExactBytesRewritten
                    | ArtifactErrorKind::UnsupportedShape
                    | ArtifactErrorKind::DigestCollision
            ),
            Self::IdentityReused => true,
            Self::Descriptor { refusal, .. } => *refusal != SourceDescriptorError::HeadMismatch,
            Self::UnadmittedSource { .. } | Self::RevisionAhead { .. } | Self::Kernel { .. } => {
                false
            }
        }
    }
}

const PRODUCER: &str = "eidnara-daemon/harness-sources";
const CAUSE: &str = "source publication";
/// Leads every request-digest input. The digest is persisted in the descriptor receipt under a key that does not change with it, so a layout change is a new tag, made deliberately, and never a silent reinterpretation of stored receipts. `v2` binds the unit's outcome record after the `v1` inputs, so a change to the fields [`crate::tool_outcome`] extracts or to the record's encoding is also a new tag, with the tag it replaces probed in [`stored_receipt`].
const REQUEST_DIGEST_FORMAT: &str = "source-request.v2";
/// The frozen tag of the layout receipts were written under before the outcome record. A key that holds a receipt under this digest replays it ([`stored_receipt`]).
const LEGACY_REQUEST_DIGEST_FORMAT: &str = "source-request.v1";

/// How far a native millisecond timestamp may lead the caller's observation and still be accepted as a revision.
pub const MAX_REVISION_LEAD_MS: i64 = 60 * 60 * 1_000;

/// How long captured harness evidence is owned after its observation: the capture replay retention, 30 days.
pub const HARNESS_CAPTURE_RETENTION_MS: i64 =
    memory_store::memory_capture::CAPTURE_REPLAY_RETENTION_MS;

/// The largest unit text capture retains: the memory store's source bound.
pub const MAX_SOURCE_UNIT_BYTES: usize = memory_store::memory_capture::MAX_CAPTURE_SOURCE_BYTES;
/// The most units one capture batch admits: the memory-capture message bound.
pub const MAX_SOURCE_BATCH_UNITS: usize = crate::memory_capture::MAX_CAPTURE_MESSAGES;
/// The most text, summed over its admitted units, one capture batch retains: the memory-capture input bound.
pub const MAX_SOURCE_BATCH_BYTES: usize = crate::memory_capture::MAX_CAPTURE_INPUT_BYTES;

/// How long `publish` waits for a kernel reader to consult the stored receipt before offering bytes to the store.
const RECEIPT_WAIT: Duration = Duration::from_secs(30);

/// The deadline a batch's receipt preview waits until: the receipt wait, or `budget`'s deadline when that is sooner.
fn preview_deadline(budget: Option<&kernel::applicability::EvalBudget>) -> Instant {
    let receipt_wait = Instant::now() + RECEIPT_WAIT;
    budget
        .and_then(kernel::applicability::EvalBudget::deadline)
        .map_or(receipt_wait, |deadline| deadline.min(receipt_wait))
}

/// `egress` controls the provider policy for retained evidence.
pub struct SourcePublisher<'a> {
    pub kernel: &'a KernelStore,
    pub domain_id: &'a str,
    /// `None` permits only units whose identities do not name a project: no project route serves unscoped rows.
    pub scope_id: Option<&'a str>,
    pub egress: ProviderEgress,
    pub sensitivity: Sensitivity,
}

impl SourcePublisher<'_> {
    /// Retains the unit's text as an exact artifact, then publishes its descriptor and records its admission in one commit. Both receipts are keyed by the unit's identity and revision; a unit published before answers from its descriptor receipt without offering its bytes to the store again. `observed_at` is the caller's Unix millisecond clock at the time it read the record.
    ///
    /// # Errors
    ///
    /// Returns identity, revision, or artifact errors before retention, descriptor errors for permanent refusals, and kernel errors including unmet preconditions such as a missing scope. Failures after retention carry the evidence object. Permanent refusals attempt retirement; kernel failures preserve evidence without implying retryability.
    /// A known scope mismatch returns `NotFound` before retention; a scope mismatch detected in the descriptor transaction preserves the retained evidence without publishing it.
    pub fn publish(&self, unit: &SourceUnit, observed_at: i64) -> Result<Published, PublishError> {
        self.publish_within(unit, observed_at, None)
    }

    /// [`Self::publish`] whose receipt preview and descriptor commit wait within `budget`; an exhausted budget refuses with [`KernelError::Deadline`] in [`PublishError::Kernel`]. Artifact retention has no budgeted entry and waits as it does for every caller.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::publish`] returns.
    pub fn publish_within(
        &self,
        unit: &SourceUnit,
        observed_at: i64,
        budget: Option<&kernel::applicability::EvalBudget>,
    ) -> Result<Published, PublishError> {
        let unit = self.prepare(unit, observed_at)?;
        let evidence_object_id = unit.evidence_object_id();
        let evidence_id = unit.evidence_id();
        // A unit published before answers from its descriptor receipt without offering its bytes to the store again; a receipt under another request is the conflict the digest exists to catch.
        let descriptor_intent = self.descriptor_intent(&unit);
        let (_, (stored, admission, evidence_digest)) = self
            .kernel
            .preview(preview_deadline(budget), |preview| {
                self.check_scope(&unit, preview)?;
                // Evidence an earlier publication retained replays under the digest that wrote it.
                let evidence_digest =
                    stored_receipt(preview, self.evidence_intent(&unit), &unit, self)?
                        .map_or_else(|| unit.request_digest.clone(), |(_, digest)| digest);
                Ok((
                    stored_descriptor(preview, descriptor_intent.clone(), &unit, self)?,
                    unit.rule.resolve(preview)?,
                    evidence_digest,
                ))
            })
            .map_err(|error| match error {
                KernelError::Conflict => PublishError::IdentityReused,
                KernelError::NotFound if unit.rule.is_inherited() => {
                    PublishError::UnadmittedSource { evidence: None }
                }
                error => PublishError::Kernel {
                    error,
                    evidence: None,
                },
            })?;
        if let Some((receipt, cited)) = stored {
            let (evidence_id, evidence_object_id) =
                cited.unwrap_or((evidence_id, evidence_object_id));
            return Ok(Published {
                object_id: receipt.result,
                occurrence_id: unit.occurrence_id,
                evidence_object_id,
                evidence_id,
                replaced_object_id: None,
                replayed: true,
                admission,
            });
        }
        let handle = self
            .kernel
            .ingest_exact_artifact(self.evidence_request(&unit, &evidence_digest))
            .map_err(|error| match error.kind() {
                ArtifactErrorKind::OperationKeyReused => PublishError::IdentityReused,
                kind => PublishError::Artifact(kind),
            })?;
        // The store classifies the evidence it retains (harness text without affirmative provenance is at least `Sensitive`), and the descriptor records that class rather than a second derivation of the rule.
        let sensitivity = match self.stored_sensitivity(&evidence_object_id) {
            Ok(sensitivity) => sensitivity,
            Err(error) => {
                let evidence = Some(RetainedEvidence {
                    object_id: evidence_object_id,
                    retired: None,
                });
                return Err(PublishError::Kernel { error, evidence });
            }
        };
        let mut refusal = None;
        let mut outcome = None;
        let operation = |envelope: &mut kernel::Envelope<'_>| {
            let (published, admission) = self.apply(
                envelope,
                &unit,
                &handle,
                sensitivity,
                observed_at,
                &mut refusal,
            )?;
            let result = published.object_id.clone();
            outcome = Some((published, admission));
            Ok(result)
        };
        let receipt = match budget {
            Some(budget) => self
                .kernel
                .commit_within_budget(budget, descriptor_intent, operation),
            None => self.kernel.commit(descriptor_intent, operation),
        };
        match (receipt, outcome) {
            (Ok(receipt), Some((published, committed_admission))) => Ok(Published {
                object_id: published.object_id,
                occurrence_id: published.occurrence_id,
                evidence_object_id,
                evidence_id: evidence_id.clone(),
                replaced_object_id: published.replaced_object_id,
                replayed: receipt.replayed,
                admission: committed_admission,
            }),
            // A receipt committed between the preview and this commit replays here; it ran no operation, and its result is the object id the first publication returned.
            (Ok(receipt), None) => Ok(Published {
                object_id: receipt.result,
                occurrence_id: unit.occurrence_id,
                evidence_object_id,
                evidence_id: evidence_id.clone(),
                replaced_object_id: None,
                replayed: true,
                admission,
            }),
            (Err(error), _) => {
                let error = match refusal {
                    Some(Err(SourceDescriptorError::Kernel(error))) => error,
                    None if error == KernelError::NotFound && unit.rule.is_inherited() => {
                        return Err(PublishError::UnadmittedSource {
                            evidence: Some(RetainedEvidence {
                                object_id: evidence_object_id,
                                retired: None,
                            }),
                        });
                    }
                    Some(Err(refusal)) => {
                        // A `HeadMismatch` can resolve as the lineage advances, so its evidence is preserved for the retry. Every other descriptor refusal is permanent and attempts to retire the retained evidence.
                        let evidence = Some(if refusal == SourceDescriptorError::HeadMismatch {
                            RetainedEvidence {
                                object_id: evidence_object_id,
                                retired: None,
                            }
                        } else {
                            self.retire_evidence(&unit.key, &unit.request_digest, &unit.origin)
                        });
                        return Err(PublishError::Descriptor { refusal, evidence });
                    }
                    _ => error,
                };
                Err(PublishError::Kernel {
                    error,
                    evidence: Some(RetainedEvidence {
                        object_id: evidence_object_id,
                        retired: None,
                    }),
                })
            }
        }
    }

    /// Judges the unit's identity, revision, and scope binding before any byte is retained, so an oversized or malformed identity refuses without partial publication.
    fn prepare<'u>(
        &self,
        unit: &'u SourceUnit,
        observed_at: i64,
    ) -> Result<Prepared<'u>, PublishError> {
        let identity: Vec<(&str, &str)> = unit
            .identity
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let encoded = encode_preserving_span(&Occurrence {
            class: unit.class.code(),
            identity: &identity,
            revision: &unit.revision,
            representation: unit.representation.as_str(),
            span: None,
        })
        .map_err(|refusal| PublishError::Descriptor {
            refusal: SourceDescriptorError::from(refusal),
            evidence: None,
        })?;
        // The outcome record is judged before any byte is retained: only a tool unit carries one, and it must describe the unit's bytes within the record bound.
        kernel::check_outcome(
            unit.outcome.as_ref(),
            unit.class,
            unit.representation.as_str(),
            unit.text.len(),
        )
        .map_err(|refusal| PublishError::Descriptor {
            refusal,
            evidence: None,
        })?;
        let rule = admission_rule(unit);
        // A native timestamp may not lead the observation; a canonical revision and a repository document's observation count are counters, not judged against the clock.
        if !rule.is_inherited()
            && unit.class != OccurrenceClass::RepositoryDocuments
            && encoded.revision > observed_at.saturating_add(MAX_REVISION_LEAD_MS)
        {
            return Err(PublishError::RevisionAhead {
                revision: encoded.revision,
                observed_at,
            });
        }
        let key = receipt_key(
            &encoded.occurrence_id,
            encoded.revision,
            unit.representation,
        );
        // Reject project-bound units without a scope before retention: no project route serves unscoped rows.
        let project = match (unit.identity_value("project_id"), self.scope_id) {
            (Some(project_id), Some(_)) => Some(ProjectScope::new(project_id).map_err(
                |error| PublishError::Kernel {
                    error,
                    evidence: None,
                },
            )?),
            (Some(_), None) => {
                return Err(PublishError::Kernel {
                    error: KernelError::InvalidInput,
                    evidence: None,
                });
            }
            (None, _) => None,
        };
        Ok(Prepared {
            unit,
            identity,
            occurrence_id: encoded.occurrence_id,
            lineage_id: encoded.lineage_id,
            revision: encoded.revision,
            rule,
            key,
            request_digest: self.request_digest(unit),
            legacy_digest: std::cell::OnceCell::new(),
            origin: unit.origin().to_owned(),
            project,
            observed_at,
        })
    }

    /// The unit's receipt key and request digest, the pair a stored receipt answers for, or `None` when its identity does not encode. Changed bytes or publishing context under one key give another digest.
    pub fn receipt_identity(&self, unit: &SourceUnit) -> Option<(String, String)> {
        Some((receipt_key_of(unit)?, self.request_digest(unit)))
    }

    fn check_scope(
        &self,
        unit: &Prepared<'_>,
        envelope: &kernel::Envelope<'_>,
    ) -> Result<(), KernelError> {
        self.check_scope_once(unit, envelope, &mut Vec::new())
    }

    /// [`Self::check_scope`] that reads each scope's terms from `envelope` at most once, keeping them in `read` for later units of the same transaction.
    fn check_scope_once<'s>(
        &'s self,
        unit: &Prepared<'_>,
        envelope: &kernel::Envelope<'_>,
        read: &mut Vec<(&'s str, Option<Vec<kernel::ScopeTermSpec>>)>,
    ) -> Result<(), KernelError> {
        match (&unit.project, self.scope_id) {
            (Some(_), Some(scope_id)) => {
                let position = match read.iter().position(|(id, _)| *id == scope_id) {
                    Some(position) => position,
                    None => {
                        read.push((scope_id, envelope.scope_terms(scope_id)?));
                        read.len() - 1
                    }
                };
                if Self::scope_admits(unit, read[position].1.as_deref()) {
                    Ok(())
                } else {
                    Err(KernelError::NotFound)
                }
            }
            _ => Ok(()),
        }
    }

    /// Whether the publisher's stored scope `terms` admit the unit's project; an absent scope row admits it here, and the descriptor commit decides it.
    fn scope_admits(unit: &Prepared<'_>, terms: Option<&[kernel::ScopeTermSpec]>) -> bool {
        match (&unit.project, terms) {
            (Some(project), Some(terms)) => project.names_project(Some(terms)),
            _ => true,
        }
    }

    fn descriptor_intent(&self, unit: &Prepared<'_>) -> CommitIntent {
        self.intent(
            &format!("source-descriptor:{}", unit.key),
            &unit.request_digest,
            &unit.origin,
        )
    }

    /// Publishes the unit's descriptor and records its admission inside `envelope`. `refusal` holds the descriptor's own refusal, and `Some(Ok(()))` once the descriptor is published.
    fn apply(
        &self,
        envelope: &mut kernel::Envelope<'_>,
        unit: &Prepared<'_>,
        handle: &ArtifactHandle,
        sensitivity: Sensitivity,
        observed_at: i64,
        refusal: &mut Option<Result<(), SourceDescriptorError>>,
    ) -> Result<(SourceDescriptorOutcome, (SourceClass, TaintClass)), KernelError> {
        self.check_scope(unit, envelope)?;
        // The classes recorded are the ones the commit itself observes, so an admission change since the preview is not carried forward.
        let (source_class, taint_class) = unit.rule.resolve(envelope)?;
        let published = envelope
            .publish_source_descriptor(&SourceDescriptorRequest {
                occurrence: Occurrence {
                    class: unit.unit.class.code(),
                    identity: &unit.identity,
                    revision: &unit.unit.revision,
                    representation: unit.unit.representation.as_str(),
                    span: None,
                },
                source_policy: unit.unit.source_policy(),
                domain_id: self.domain_id,
                scope_id: self.scope_id,
                evidence_id: &handle.evidence_id,
                artifact_digest: &handle.digest,
                buffer: &unit.unit.text,
                sensitivity,
                observed_at,
                expected_head: unit.expected_head(),
                outcome: unit.unit.outcome.as_ref(),
            })
            .map_err(|error| {
                *refusal = Some(Err(error));
                KernelError::InvalidInput
            })?;
        *refusal = Some(Ok(()));
        envelope.record_admission(AdmissionRequest {
            candidate_id: None,
            subject_object_id: Some(published.object_id.clone()),
            source_class: Some(source_class),
            taint_class: Some(taint_class),
            event: AdmissionEvent {
                kind: EventKind::Other,
                trigger_object_id: unit.rule.subject().map(str::to_owned),
                approval_object_id: None,
                evidence_id: Some(handle.evidence_id.clone()),
                reason: CAUSE.to_owned(),
            },
        })?;
        Ok((published, (source_class, taint_class)))
    }

    fn evidence_intent(&self, unit: &Prepared<'_>) -> CommitIntent {
        self.intent(
            &format!("source-evidence:{}", unit.key),
            &unit.request_digest,
            &unit.origin,
        )
    }

    /// The evidence ingest for `unit`. Captured message text and tool results are `harness_capture` evidence owned for [`HARNESS_CAPTURE_RETENTION_MS`] from `observed_at`; every other class is `canonical` and never ages out. The deadline is set only when the evidence row is first created: the evidence receipt is keyed by the unit's identity and revision and its request digest excludes the clock, so a later attempt of the same unit replays the first row and its deadline.
    fn evidence_request(&self, unit: &Prepared<'_>, request_digest: &str) -> ArtifactIngestRequest {
        let (retention_class, retain_until) = match unit.unit.class {
            OccurrenceClass::Messages | OccurrenceClass::RawToolSpans => (
                kernel::HARNESS_CAPTURE_RETENTION_CLASS,
                Some(
                    unit.observed_at
                        .saturating_add(HARNESS_CAPTURE_RETENTION_MS),
                ),
            ),
            _ => ("canonical", None),
        };
        ArtifactIngestRequest {
            intent: CommitIntent {
                request_digest: request_digest.to_owned(),
                ..self.evidence_intent(unit)
            },
            payload: unit.unit.text.as_bytes().to_vec(),
            evidence_id: unit.evidence_id(),
            object_id: unit.evidence_object_id(),
            object_kind: "evidence".to_owned(),
            domain_id: self.domain_id.to_owned(),
            source_kind: unit.unit.class.code().to_owned(),
            source_id: format!("{}:{}", unit.origin, unit.key),
            source_revision: unit.revision,
            media_type: "text/plain".to_owned(),
            retention_class: retention_class.to_owned(),
            retain_until,
            asserted_sensitivity: self.sensitivity,
            provider_egress: self.egress,
            // Only the git publisher's commit provenance is retained; this publisher constructs none.
            provenance: crate::git_sources::commit_provenance(unit.unit),
        }
    }

    /// The class the store recorded for the evidence object at ingest.
    fn stored_sensitivity(&self, evidence_object_id: &str) -> Result<Sensitivity, KernelError> {
        let (_, states) = self
            .kernel
            .object_states(std::slice::from_ref(&evidence_object_id.to_owned()))?;
        states
            .into_iter()
            .next()
            .flatten()
            .map(|state| state.object.sensitivity)
            .ok_or(KernelError::NotFound)
    }

    /// An already-invalidated object counts as retired: a replayed artifact receipt for an already-compensated unit names exactly that object.
    fn retire_evidence(&self, key: &str, request_digest: &str, harness: &str) -> RetainedEvidence {
        let object_id = format!("srcev-object:{key}");
        let retired = self
            .kernel
            .commit(
                self.intent(
                    &format!("source-evidence-retire:{key}"),
                    request_digest,
                    harness,
                ),
                |envelope| {
                    envelope.retire_evidence(&object_id)?;
                    Ok(String::new())
                },
            )
            .map(|_| ());
        let retired = match retired {
            Err(KernelError::NotFound) => {
                match self.kernel.object_states(std::slice::from_ref(&object_id)) {
                    Ok((_, states))
                        if states[0]
                            .as_ref()
                            .is_some_and(|state| state.object.invalidated_commit_seq.is_some()) =>
                    {
                        Ok(())
                    }
                    Ok(_) => Err(KernelError::NotFound),
                    Err(error) => Err(error),
                }
            }
            other => other,
        };
        RetainedEvidence {
            object_id,
            retired: Some(retired),
        }
    }

    fn intent(&self, operation: &str, request_digest: &str, harness: &str) -> CommitIntent {
        CommitIntent {
            producer: PRODUCER.to_owned(),
            operation_key: operation.to_owned(),
            request_digest: request_digest.to_owned(),
            actor: harness.to_owned(),
            cause: CAUSE.to_owned(),
        }
    }

    /// The request digest binds text, domain, scope, role, sensitivity, provider-egress policy, and the outcome record. Changing any of these under the same receipt key conflicts rather than replaying.
    pub(crate) fn request_digest(&self, unit: &SourceUnit) -> String {
        let mut digest = self.digest_inputs(unit, REQUEST_DIGEST_FORMAT);
        match &unit.outcome {
            Some(outcome) => {
                let encoded = outcome.encode();
                digest.update([1]);
                digest.update((encoded.len() as u64).to_be_bytes());
                digest.update(&encoded);
            }
            None => digest.update([0]),
        }
        lower_hex(&digest.finalize())
    }

    /// The `source-request.v1` digest of `unit`: the layout before the outcome record, frozen so receipts written under it still replay.
    fn legacy_request_digest(&self, unit: &SourceUnit) -> String {
        lower_hex(
            &self
                .digest_inputs(unit, LEGACY_REQUEST_DIGEST_FORMAT)
                .finalize(),
        )
    }

    /// The SHA-256 state over the inputs both digest layouts share, led by `format`, each streamed into the hash in place after its big-endian `u64` length, so the unit's text is read once and never copied. The `v1` layout is these bytes alone, so they are frozen: a change here moves the legacy digest too.
    fn digest_inputs(&self, unit: &SourceUnit, format: &str) -> Sha256 {
        let mut digest = Sha256::new();
        for part in [
            format,
            unit.text.as_str(),
            self.domain_id,
            self.scope_id.unwrap_or_default(),
            unit.role.as_str(),
            self.sensitivity.as_str(),
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part.as_bytes());
        }
        digest.update([
            u8::from(self.scope_id.is_some()),
            match self.egress {
                ProviderEgress::RemoteAllowed => 0,
                ProviderEgress::LocalOnly => 1,
            },
        ]);
        digest
    }
}

/// The receipt `preview` holds for `intent`'s key and the digest it was written under: `intent`'s digest, or else the `source-request.v1` digest of `unit` under `publisher`, which is computed only when the key holds a receipt under another digest. A receipt written before the outcome record replays as it was written, without enrichment. A key holding neither digest is [`KernelError::Conflict`].
fn stored_receipt(
    preview: &kernel::Envelope<'_>,
    intent: CommitIntent,
    unit: &Prepared<'_>,
    publisher: &SourcePublisher<'_>,
) -> Result<Option<(kernel::CommitReceipt, String)>, KernelError> {
    match preview.stored_receipt(intent.clone()) {
        Err(KernelError::Conflict) => {
            let legacy_digest = unit.legacy_digest(publisher);
            preview
                .stored_receipt(CommitIntent {
                    request_digest: legacy_digest.to_owned(),
                    ..intent
                })?
                .map(|receipt| Some((receipt, legacy_digest.to_owned())))
                .ok_or(KernelError::Conflict)
        }
        other => Ok(other?.map(|receipt| (receipt, intent.request_digest))),
    }
}

/// Identity conflicts and descriptor replays take precedence over purge refusal, which applies only when the commit would ingest the unit's bytes.
fn readmission_blocked(
    preview: &kernel::Preview<'_>,
    publisher: &SourcePublisher<'_>,
    unit: &Prepared<'_>,
) -> Result<bool, KernelError> {
    match stored_receipt(preview, publisher.descriptor_intent(unit), unit, publisher) {
        Ok(None) => {}
        Ok(Some(_)) | Err(KernelError::Conflict) => return Ok(false),
        Err(error) => return Err(error),
    }
    if !preview.artifact_readmission_blocked(&kernel::source_identity::payload_id(
        unit.unit.text.as_bytes(),
    ))? {
        return Ok(false);
    }
    match stored_receipt(preview, publisher.evidence_intent(unit), unit, publisher) {
        Ok(_) => Ok(true),
        Err(KernelError::Conflict) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Both receipts of `unit` share this key, or `None` when its identity does not encode.
pub(crate) fn receipt_key_of(unit: &SourceUnit) -> Option<String> {
    let identity: Vec<(&str, &str)> = unit
        .identity
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let (occurrence_id, revision) = occurrence_id_preserving_span(&Occurrence {
        class: unit.class.code(),
        identity: &identity,
        revision: &unit.revision,
        representation: unit.representation.as_str(),
        span: None,
    })
    .ok()?;
    Some(receipt_key(&occurrence_id, revision, unit.representation))
}

/// Both receipts of a unit are keyed by its occurrence, revision, and representation: the digest of `occurrence_id`, the decimal revision, and the representation, joined by U+001F.
fn receipt_key(occurrence_id: &str, revision: i64, representation: Representation) -> String {
    let mut hasher = Sha256::new();
    hasher.update(occurrence_id.as_bytes());
    hasher.update([0x1f]);
    hasher.update(revision.to_string());
    hasher.update([0x1f]);
    hasher.update(representation.as_str());
    lower_hex(&hasher.finalize())
}

/// Writes inside a commit and returns the result recorded under the write's key.
pub type EnvelopeWrite<'a> =
    Box<dyn FnOnce(&mut kernel::Envelope<'_>) -> Result<String, KernelError> + 'a>;

/// A write a [`publish_batch`] commits beside its publications under its own key.
pub struct KeyedWrite<'a> {
    pub intent: CommitIntent,
    pub write: EnvelopeWrite<'a>,
}

/// Commits `writes` in order and then publishes every unit of `units` under its publisher, all in one kernel commit, and returns each unit's outcome in order.
///
/// A write or unit whose receipt is already recorded is skipped, and a skipped unit reports a replayed publication. The commit records every other write's and unit's own receipt, and every retained text's evidence receipt, so a later commit of any one of them alone replays. Units of one publisher with equal class and text cite one evidence object, and a unit whose evidence an earlier publication retained cites that evidence. A unit's descriptor carries the class the store recorded for its evidence, and its admission classes are read in the commit.
///
/// # Errors
///
/// Returns the kernel's refusal of the batch, including any unit [`SourcePublisher::publish_within`] would refuse and an exhausted `budget`. Nothing commits. The refusal names the unit whose identity, scope, stored receipt, or descriptor refused it. A receipt stored under another request digest for the unit's key, or an earlier unit of the batch under the same key with another request, is [`KernelError::Conflict`] naming that unit.
pub fn publish_batch(
    kernel: &KernelStore,
    writes: Vec<KeyedWrite<'_>>,
    units: &[(&SourcePublisher<'_>, &SourceUnit)],
    observed_at: i64,
    budget: Option<&kernel::applicability::EvalBudget>,
) -> Result<Vec<Published>, BatchRefusal> {
    let mut prepared = Vec::with_capacity(units.len());
    for (index, (publisher, unit)) in units.iter().enumerate() {
        prepared.push(
            publisher
                .prepare(unit, observed_at)
                .map_err(|_| BatchRefusal {
                    error: KernelError::InvalidInput,
                    unit: Some(index),
                    refusal: None,
                })?,
        );
    }
    publish_prepared(kernel, writes, units, &prepared, observed_at, budget)
}

/// [`publish_batch`] of `units` whose publishers already prepared them, in order, as `prepared`.
fn publish_prepared(
    kernel: &KernelStore,
    writes: Vec<KeyedWrite<'_>>,
    units: &[(&SourcePublisher<'_>, &SourceUnit)],
    prepared: &[Prepared<'_>],
    observed_at: i64,
    budget: Option<&kernel::applicability::EvalBudget>,
) -> Result<Vec<Published>, BatchRefusal> {
    // Two units under one key with different requests would claim one receipt twice in this commit; the later one is the conflict.
    let mut digests = std::collections::HashMap::with_capacity(prepared.len());
    for (index, unit) in prepared.iter().enumerate() {
        if let Some(digest) = digests.insert(unit.key.as_str(), unit.request_digest.as_str())
            && digest != unit.request_digest
        {
            return Err(BatchRefusal {
                error: KernelError::Conflict,
                unit: Some(index),
                refusal: None,
            });
        }
    }
    let preview_deadline = preview_deadline(budget);
    let mut culprit = None;
    let (_, (written, previewed)) = kernel
        .preview(preview_deadline, |preview| {
            let written = writes
                .iter()
                .map(|write| Ok(preview.stored_receipt(write.intent.clone())?.is_some()))
                .collect::<Result<Vec<bool>, KernelError>>()?;
            let mut previewed = Vec::with_capacity(units.len());
            let mut scopes = Vec::new();
            for (index, ((publisher, _), unit)) in units.iter().zip(prepared).enumerate() {
                let mut step = || {
                    publisher.check_scope_once(unit, preview, &mut scopes)?;
                    Ok((
                        stored_descriptor(
                            preview,
                            publisher.descriptor_intent(unit),
                            unit,
                            publisher,
                        )?,
                        unit.rule.resolve(preview)?,
                        stored_receipt(preview, publisher.evidence_intent(unit), unit, publisher)?
                            .map(|(receipt, _)| receipt),
                    ))
                };
                previewed.push(step().inspect_err(|_| culprit = Some(index))?);
            }
            Ok((written, previewed))
        })
        .map_err(|error| BatchRefusal {
            error,
            unit: culprit,
            refusal: None,
        })?;
    let writes: Vec<KeyedWrite<'_>> = writes
        .into_iter()
        .zip(written)
        .filter_map(|(write, written)| (!written).then_some(write))
        .collect();
    let mut outcomes: Vec<Option<Published>> = Vec::with_capacity(units.len());
    let mut pending = Vec::new();
    let mut retained = std::collections::HashMap::new();
    for (index, (unit, (stored, admission, evidence))) in prepared.iter().zip(previewed).enumerate()
    {
        // Evidence an earlier attempt retained without publishing its descriptor is cited again: exact retention stores the unit's bytes under their own digest.
        if let Some(evidence) = evidence.filter(|_| stored.is_none()) {
            retained.insert(
                index,
                ArtifactHandle {
                    digest: kernel::source_identity::payload_id(units[index].1.text.as_bytes()),
                    evidence_id: evidence.result,
                },
            );
        }
        match stored {
            Some((receipt, cited)) => {
                let (evidence_id, evidence_object_id) =
                    cited.unwrap_or_else(|| (unit.evidence_id(), unit.evidence_object_id()));
                outcomes.push(Some(Published {
                    object_id: receipt.result,
                    occurrence_id: unit.occurrence_id.clone(),
                    evidence_object_id,
                    evidence_id,
                    replaced_object_id: None,
                    replayed: true,
                    admission,
                }));
            }
            None => {
                outcomes.push(None);
                pending.push(index);
            }
        }
    }
    let main = match (writes.first(), pending.first()) {
        (Some(write), _) => write.intent.clone(),
        (None, Some(&index)) => units[index].0.descriptor_intent(&prepared[index]),
        (None, None) => {
            return Ok(outcomes
                .into_iter()
                .map(|outcome| outcome.expect("replayed"))
                .collect());
        }
    };
    let owners: Vec<usize> = pending
        .iter()
        .map(|&index| {
            *pending
                .iter()
                .find(|&&owner| {
                    std::ptr::eq(units[owner].0, units[index].0)
                        && units[owner].1.class == units[index].1.class
                        && units[owner].1.text == units[index].1.text
                })
                .expect("a unit owns its own text")
        })
        .collect();
    let mut requests = Vec::new();
    let mut request_of = std::collections::HashMap::new();
    for (&index, &owner) in pending.iter().zip(&owners) {
        if owner == index && !retained.contains_key(&index) {
            request_of.insert(index, requests.len());
            requests.push(
                units[index]
                    .0
                    .evidence_request(&prepared[index], &prepared[index].request_digest),
            );
        }
    }
    let main_is_write = !writes.is_empty();
    let mut published: Vec<Published> = Vec::with_capacity(pending.len());
    let mut culprit = None;
    let receipt = kernel
        .ingest_exact_artifacts_with(main, requests, budget, |envelope, handles| {
            let mut main_result = None;
            for (position, write) in writes.into_iter().enumerate() {
                let intent = write.intent;
                let result = (write.write)(envelope)?;
                if position == 0 {
                    main_result = Some(result);
                } else {
                    envelope.record_receipt(intent, &result)?;
                }
            }
            for (&index, &owner) in pending.iter().zip(&owners) {
                let (publisher, _) = units[index];
                let unit = &prepared[index];
                let handle = retained
                    .get(&owner)
                    .unwrap_or_else(|| &handles[request_of[&owner]]);
                let evidence_object_id = prepared[owner].evidence_object_id();
                let records_receipt = main_is_write || !published.is_empty();
                let mut refusal = None;
                // The error carries whether the unit's own receipt write refused.
                let mut step = || {
                    let sensitivity = envelope
                        .object_state(&evidence_object_id)
                        .and_then(|state| state.ok_or(KernelError::NotFound))
                        .map_err(|error| (error, false))?
                        .object
                        .sensitivity;
                    let (descriptor, admission) = publisher
                        .apply(
                            envelope,
                            unit,
                            handle,
                            sensitivity,
                            observed_at,
                            &mut refusal,
                        )
                        .map_err(|error| (error, false))?;
                    if records_receipt {
                        envelope
                            .record_receipt(
                                publisher.descriptor_intent(unit),
                                &descriptor.object_id,
                            )
                            .map_err(|error| (error, true))?;
                    }
                    Ok((descriptor, admission))
                };
                // A conflict on the unit's own receipt is another writer's commit since the preview, possibly of these very bytes; the next attempt's preview tells a replay from a conflict.
                let (descriptor, admission) = step().map_err(|(error, receipt)| {
                    if !(receipt && error == KernelError::Conflict) {
                        culprit = Some((
                            index,
                            refusal.and_then(Result::err).filter(|refusal| {
                                !matches!(refusal, SourceDescriptorError::Kernel(_))
                            }),
                        ));
                    }
                    error
                })?;
                if !records_receipt {
                    main_result = Some(descriptor.object_id.clone());
                }
                published.push(Published {
                    object_id: descriptor.object_id,
                    occurrence_id: descriptor.occurrence_id,
                    evidence_object_id,
                    evidence_id: handle.evidence_id.clone(),
                    replaced_object_id: descriptor.replaced_object_id,
                    replayed: false,
                    admission,
                });
            }
            Ok(main_result.expect("the batch commits a write or a unit"))
        })
        .map_err(|error| BatchRefusal {
            error,
            unit: culprit.map(|(index, _)| index),
            refusal: culprit.and_then(|(_, refusal)| refusal),
        })?;
    // A receipt committed under the main key since the preview ran no operation.
    if receipt.replayed || published.len() != pending.len() {
        return Err(BatchRefusal {
            error: KernelError::Conflict,
            unit: None,
            refusal: None,
        });
    }
    for (index, published) in pending.into_iter().zip(published) {
        outcomes[index] = Some(published);
    }
    Ok(outcomes
        .into_iter()
        .map(|outcome| outcome.expect("every unit has an outcome"))
        .collect())
}

/// A batch attempt that committed nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the batch committed nothing: {error}")]
pub struct BatchRefusal {
    /// The cause: [`KernelError::Conflict`] when `unit`'s key already holds a receipt under another request digest.
    pub error: KernelError,
    /// The index in the caller's units whose identity validation, scope check, receipt lookup, or descriptor commit failed. The cause can be transient, such as [`KernelError::Deadline`] or [`KernelError::Busy`]. `None` identifies a refusal involving the batch, one of its writes, or a receipt another writer committed after this attempt's preview.
    pub unit: Option<usize>,
    /// The descriptor refusal of `unit` inside the commit, such as [`SourceDescriptorError::RevisionNotAdvanced`]. A kernel failure inside descriptor publication leaves this `None` and is `error`.
    pub refusal: Option<SourceDescriptorError>,
}

impl BatchRefusal {
    /// The unit whose key holds a receipt under another request digest: changed bytes or publishing context under one identity and revision. Unchanged input refuses the same way on every attempt, so the caller marks that unit terminal and offers its siblings again.
    pub fn conflicting_unit(&self) -> Option<usize> {
        self.unit.filter(|_| self.error == KernelError::Conflict)
    }
}

/// What a screened capture batch did with one unit.
#[derive(Debug)]
pub enum UnitDisposition {
    /// Published in the batch's commit, or answered from a stored receipt.
    Published(Published),
    /// The text exceeds [`MAX_SOURCE_UNIT_BYTES`]. Nothing was retained, and unchanged input refuses again.
    UnitTooLarge,
    /// The batch reached its unit or byte bound first, or an admitted unit shares the unit's lineage. The unit remains available for a later pass to offer again.
    Deferred,
    /// The publisher rejects the unit's identity, revision, or scope binding before the transaction, or the commit rejects its descriptor and [`publish_screened`] retries the batch without it. [`PublishError::is_permanent`] indicates whether unchanged input will be rejected again.
    Refused(PublishError),
    /// A tool result whose call has a newer generation live or in this batch, and which was never published. Nothing was retained, and the newer generation stays the call's only live one.
    Superseded,
}

/// Screens `units` for one capture batch of `publisher`, then publishes the admitted units in one kernel commit and returns one disposition per unit, in order.
///
/// [`publish_screened`] reconciles tool results by call (project, harness, session, parent message, tool call). Each call's newest generation in the batch retires its older live generations in the same commit; `Reruns::apply` validates the retirement plan inside that commit. Stored-receipt replays keep their recorded disposition; other units older than the call's head are [`UnitDisposition::Superseded`]. Stale and first-generation decisions remain valid when every publication to the store's `raw_tool_spans` is serialized from preview through commit. Capture's commit gate provides this serialization. [`SourcePublisher::publish`] and [`publish_batch`] publish tool units as given.
///
/// Screening keeps the units' order. A unit over [`MAX_SOURCE_UNIT_BYTES`] is [`UnitDisposition::UnitTooLarge`]. A unit [`SourcePublisher::publish`] would refuse before retention, or whose project the publisher's stored scope does not name, is [`UnitDisposition::Refused`]. Neither counts toward the batch bounds. The remaining units are admitted until the next one would exceed [`MAX_SOURCE_BATCH_UNITS`] units or [`MAX_SOURCE_BATCH_BYTES`] of text; that unit and every later unit within the size cap are [`UnitDisposition::Deferred`] without further screening. A unit within the bounds whose text exact retention refuses, such as text holding a recognized secret, is [`UnitDisposition::Refused`] with that [`PublishError::Artifact`] kind and takes no admission slot. A unit within the bounds whose key holds no stored descriptor receipt, whose evidence receipt is absent or matches its request, and whose payload digest a purge tombstone or a pending unlink names is [`UnitDisposition::Refused`] with [`ArtifactErrorKind::ReAdmissionBlocked`] and takes no admission slot; a digest under reclamation alone reaches ingest, which refuses it retryably. A unit that replays a stored receipt counts toward the batch bounds. A later unit of an admitted unit's lineage is [`UnitDisposition::Deferred`], except a changed request under the admitted unit's key, which is [`UnitDisposition::Refused`] with [`PublishError::IdentityReused`]. When the commit refuses an admitted unit's descriptor, that unit is [`UnitDisposition::Refused`] with [`PublishError::Descriptor`], and the remaining admitted units are committed in another attempt. A batch with no admitted unit commits nothing. Units are prepared and scanned with no kernel reader held. The scope is read in one preview, and the admitted units' receipts and re-admission states are read together in another; when that read blocks a unit, screening resumes at the first blocked unit with its slot released, and reads only the units it newly admits. The screening reads together, and the batch's receipt preview, each wait at most the receipt wait, or until `budget`'s deadline.
///
/// # Errors
///
/// A screening read's failure returns with no unit. A refusal from [`publish_batch`] identifies the admitted unit by its position in `units`.
pub fn publish_screened(
    publisher: &SourcePublisher<'_>,
    units: &[SourceUnit],
    observed_at: i64,
    budget: Option<&kernel::applicability::EvalBudget>,
) -> Result<Vec<UnitDisposition>, BatchRefusal> {
    publish_screened_between(publisher, units, observed_at, budget, || {})
}

/// [`publish_screened`] that runs `between` after screening and before the batch commit begins.
pub(crate) fn publish_screened_between(
    publisher: &SourcePublisher<'_>,
    units: &[SourceUnit],
    observed_at: i64,
    budget: Option<&kernel::applicability::EvalBudget>,
    between: impl FnOnce(),
) -> Result<Vec<UnitDisposition>, BatchRefusal> {
    let mut dispositions: Vec<Option<UnitDisposition>> = Vec::with_capacity(units.len());
    let mut terms: Option<Option<Vec<kernel::ScopeTermSpec>>> = None;
    let mut admitted = Vec::new();
    let mut admitted_prepared: Vec<Prepared<'_>> = Vec::new();
    let mut admitted_lineages: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut admitted_bytes = 0usize;
    let mut full = false;
    let deadline = preview_deadline(budget);
    let screening_refusal = |error| BatchRefusal {
        error,
        unit: None,
        refusal: None,
    };
    let mut blocked: Vec<Option<bool>> = vec![None; units.len()];
    let mut next = 0;
    loop {
        let mut unchecked = Vec::new();
        for (index, unit) in units.iter().enumerate().skip(next) {
            let len = unit.text.len();
            if len > MAX_SOURCE_UNIT_BYTES {
                dispositions.push(Some(UnitDisposition::UnitTooLarge));
                continue;
            }
            if full {
                dispositions.push(Some(UnitDisposition::Deferred));
                continue;
            }
            let prepared = match publisher.prepare(unit, observed_at) {
                Ok(prepared) => prepared,
                Err(refusal) => {
                    dispositions.push(Some(UnitDisposition::Refused(refusal)));
                    continue;
                }
            };
            if let (Some(_), Some(scope_id)) = (&prepared.project, publisher.scope_id) {
                if terms.is_none() {
                    let (_, read) = publisher
                        .kernel
                        .preview(deadline, |preview| preview.scope_terms(scope_id))
                        .map_err(screening_refusal)?;
                    terms = Some(read);
                }
                if !SourcePublisher::scope_admits(
                    &prepared,
                    terms.as_ref().and_then(Option::as_deref),
                ) {
                    dispositions.push(Some(UnitDisposition::Refused(PublishError::Kernel {
                        error: KernelError::NotFound,
                        evidence: None,
                    })));
                    continue;
                }
            }
            // Each commit publishes at most one revision per lineage, so additional nonconflicting units in an admitted lineage wait for another pass.
            if let Some((key, digest)) = admitted_lineages.get(prepared.lineage_id.as_str()) {
                dispositions.push(Some(
                    if *key == prepared.key && *digest != prepared.request_digest {
                        UnitDisposition::Refused(PublishError::IdentityReused)
                    } else {
                        UnitDisposition::Deferred
                    },
                ));
                continue;
            }
            if admitted.len() == MAX_SOURCE_BATCH_UNITS
                || admitted_bytes + len > MAX_SOURCE_BATCH_BYTES
            {
                full = true;
                dispositions.push(Some(UnitDisposition::Deferred));
                continue;
            }
            if let Some(kind) = kernel::exact_text_refusal(&unit.text) {
                dispositions.push(Some(UnitDisposition::Refused(PublishError::Artifact(kind))));
                continue;
            }
            match blocked[index] {
                Some(true) => {
                    dispositions.push(Some(UnitDisposition::Refused(PublishError::Artifact(
                        ArtifactErrorKind::ReAdmissionBlocked,
                    ))));
                    continue;
                }
                Some(false) => {}
                None => unchecked.push(admitted.len()),
            }
            admitted_lineages.insert(
                prepared.lineage_id.clone(),
                (prepared.key.clone(), prepared.request_digest.clone()),
            );
            admitted_prepared.push(prepared);
            admitted.push(index);
            admitted_bytes += len;
            dispositions.push(None);
        }
        if unchecked.is_empty() {
            break;
        }
        let (_, answers) = publisher
            .kernel
            .preview(deadline, |preview| {
                unchecked
                    .iter()
                    .map(|&position| {
                        readmission_blocked(preview, publisher, &admitted_prepared[position])
                    })
                    .collect::<Result<Vec<bool>, KernelError>>()
            })
            .map_err(screening_refusal)?;
        for (&position, &answer) in unchecked.iter().zip(&answers) {
            blocked[admitted[position]] = Some(answer);
        }
        let Some(first) = unchecked
            .iter()
            .zip(&answers)
            .find_map(|(&position, &answer)| answer.then_some(position))
        else {
            break;
        };
        next = admitted[first];
        dispositions.truncate(next);
        admitted.truncate(first);
        admitted_prepared.truncate(first);
        admitted_lineages = admitted_prepared
            .iter()
            .map(|prepared| {
                (
                    prepared.lineage_id.clone(),
                    (prepared.key.clone(), prepared.request_digest.clone()),
                )
            })
            .collect();
        admitted_bytes = admitted.iter().map(|&index| units[index].text.len()).sum();
        full = false;
    }
    between();
    // Each retry removes one admitted unit, so publish attempts are bounded by the initial admitted count. Each attempt plans the batch's re-runs over the units it still holds, so a removed head generation retires nothing.
    while !admitted.is_empty() {
        let (reconciled, reruns) =
            plan_reruns(publisher, &admitted_prepared, budget).map_err(|error| BatchRefusal {
                error,
                unit: None,
                refusal: None,
            })?;
        let mut position = 0;
        for reconciled in reconciled {
            match reconciled {
                Reconciled::Kept => position += 1,
                Reconciled::Superseded | Reconciled::Conflicting => {
                    admitted_prepared.remove(position);
                    dispositions[admitted.remove(position)] = Some(match reconciled {
                        Reconciled::Superseded => UnitDisposition::Superseded,
                        _ => UnitDisposition::Refused(PublishError::IdentityReused),
                    });
                }
            }
        }
        let writes: Vec<KeyedWrite<'_>> = reruns
            .map(|reruns| KeyedWrite {
                intent: reruns.intent(),
                write: Box::new(move |envelope: &mut kernel::Envelope<'_>| {
                    reruns.apply(publisher.kernel, envelope)
                }),
            })
            .into_iter()
            .collect();
        if admitted.is_empty() {
            break;
        }
        let batch: Vec<(&SourcePublisher<'_>, &SourceUnit)> = admitted
            .iter()
            .map(|&index| (publisher, &units[index]))
            .collect();
        match publish_prepared(
            publisher.kernel,
            writes,
            &batch,
            &admitted_prepared,
            observed_at,
            budget,
        ) {
            Ok(published) => {
                for (index, published) in admitted.drain(..).zip(published) {
                    dispositions[index] = Some(UnitDisposition::Published(published));
                }
            }
            Err(BatchRefusal {
                unit: Some(position),
                refusal: Some(refusal),
                ..
            }) => {
                admitted_prepared.remove(position);
                dispositions[admitted.remove(position)] =
                    Some(UnitDisposition::Refused(PublishError::Descriptor {
                        refusal,
                        evidence: None,
                    }));
            }
            Err(refusal) => {
                return Err(BatchRefusal {
                    unit: refusal.unit.map(|position| admitted[position]),
                    ..refusal
                });
            }
        }
    }
    Ok(dispositions
        .into_iter()
        .map(|disposition| disposition.expect("every unit has a disposition"))
        .collect())
}

/// Live descriptors one inventory page reads.
const INVENTORY_PAGE_ROWS: usize = 2048;
/// Leads the bytes a re-run retirement's receipt key and digest bind, so a change to grouping is a new tag.
const RERUN_RETIREMENT_FORMAT: &str = "source-rerun-retire.v1";

/// One native tool call: the `raw_tool_spans` identity fields that name the call, without `result_revision` and `block_index`. A re-run of the call is a newer generation of it; a new `tool_call_id` is another call.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ToolCall {
    project_id: String,
    harness: String,
    session_id: String,
    parent_message_id: String,
    tool_call_id: String,
}

impl ToolCall {
    fn fields(&self) -> [&str; 5] {
        [
            &self.project_id,
            &self.harness,
            &self.session_id,
            &self.parent_message_id,
            &self.tool_call_id,
        ]
    }
}

/// The call and generation (`result_revision`) a `raw_tool_spans` identity names, each field read by name. `None` when a field is missing or the generation is not a decimal integer.
fn tool_generation<'a>(
    identity: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<(ToolCall, i64)> {
    let fields: HashMap<&str, &str> = identity.into_iter().collect();
    let field = |name| fields.get(name).map(|value| (*value).to_owned());
    Some((
        ToolCall {
            project_id: field("project_id")?,
            harness: field("harness")?,
            session_id: field("session_id")?,
            parent_message_id: field("parent_message_id")?,
            tool_call_id: field("tool_call_id")?,
        },
        fields.get("result_revision")?.parse().ok()?,
    ))
}

/// Each tool call's live descriptor object ids with their generations.
type LiveGenerations = HashMap<ToolCall, Vec<(String, i64)>>;

/// The live generations of each of `calls` in `envelope`'s view: one pass over the `raw_tool_spans` inventory, narrowed in the store to the calls' `tool_call_id`s.
///
/// # Errors
///
/// Returns [`KernelError::Deadline`] when `budget` is exhausted before a page and [`KernelError::CorruptCanonicalRow`] when a live call row has an unparseable generation. Propagates inventory errors.
fn live_generations(
    envelope: &kernel::Envelope<'_>,
    calls: &HashSet<ToolCall>,
    budget: Option<&kernel::applicability::EvalBudget>,
) -> Result<LiveGenerations, KernelError> {
    let mut tool_call_ids: Vec<&str> = calls
        .iter()
        .map(|call| call.tool_call_id.as_str())
        .collect();
    tool_call_ids.sort_unstable();
    tool_call_ids.dedup();
    let mut live = LiveGenerations::new();
    let mut after = None;
    loop {
        if budget.is_some_and(kernel::applicability::EvalBudget::is_exhausted) {
            return Err(KernelError::Deadline);
        }
        let page = envelope.live_source_descriptors_with(
            OccurrenceClass::RawToolSpans,
            "tool_call_id",
            &tool_call_ids,
            after.as_deref(),
            std::num::NonZeroUsize::new(INVENTORY_PAGE_ROWS).expect("a nonzero page"),
        )?;
        for row in page.rows {
            let identity = row.detail.identity.iter();
            match tool_generation(identity.map(|(name, value)| (name.as_str(), value.as_str()))) {
                Some((call, generation)) if calls.contains(&call) => {
                    live.entry(call)
                        .or_default()
                        .push((row.object_id, generation));
                }
                Some(_) => {}
                None => return Err(KernelError::CorruptCanonicalRow),
            }
        }
        match page.next {
            Some(next) => after = Some(next),
            None => return Ok(live),
        }
    }
}

/// The live predecessors of a call whose successor is `successor`: its live generations older than the successor, sorted and deduplicated. `None` when a newer generation is live, so the successor is stale.
fn predecessors(live: &[(String, i64)], successor: i64) -> Option<Vec<String>> {
    if live.iter().any(|(_, generation)| *generation > successor) {
        return None;
    }
    let mut retired: Vec<String> = live
        .iter()
        .filter(|(_, generation)| *generation < successor)
        .map(|(object_id, _)| object_id.clone())
        .collect();
    retired.sort();
    retired.dedup();
    Some(retired)
}

/// The retirement one tool re-run commits beside its successor: the call, the successor generation, and the predecessor descriptors it retires, sorted and deduplicated. A batch commits its re-runs together ([`Reruns`]).
struct Rerun {
    call: ToolCall,
    successor: i64,
    retired: Vec<String>,
}

/// `Reruns` groups a batch's retirements into one keyed write and records the snapshot tip used to plan them, with the store incarnation read before that snapshot.
struct Reruns {
    snapshot: i64,
    incarnation: kernel::CommitReadIncarnation,
    budget: Option<kernel::applicability::EvalBudget>,
    reruns: Vec<Rerun>,
}

impl Reruns {
    /// The intent derives the batch's key and fingerprint from every call, successor generation, and retirement set in call order under a versioned tag. Identical replacement batches share a key across snapshots.
    fn intent(&self) -> CommitIntent {
        let mut sorted: Vec<&Rerun> = self.reruns.iter().collect();
        sorted.sort_by(|a, b| a.call.fields().cmp(&b.call.fields()));
        let mut bytes = Vec::new();
        let mut part = |text: &str| {
            bytes.extend_from_slice(&(text.len() as u64).to_be_bytes());
            bytes.extend_from_slice(text.as_bytes());
        };
        part(RERUN_RETIREMENT_FORMAT);
        for rerun in &sorted {
            rerun.call.fields().into_iter().for_each(&mut part);
            part(&rerun.successor.to_string());
            part(&rerun.retired.len().to_string());
            rerun.retired.iter().for_each(|object_id| part(object_id));
        }
        let digest = identity_digest(&bytes);
        CommitIntent {
            producer: PRODUCER.to_owned(),
            operation_key: format!("source-rerun-retire:{digest}"),
            request_digest: digest,
            actor: sorted
                .first()
                .map(|rerun| rerun.call.harness.clone())
                .unwrap_or_default(),
            cause: "tool re-run retirement".to_owned(),
        }
    }

    /// Retires the planned predecessors. When the commit's sequence differs from `snapshot + 1`, or a restore changed `kernel`'s incarnation since the plan, it first reads every call's live generations again in one pass and requires each call's generations to be at most its successor with a predecessor set equal to `retired`; a mismatch returns [`KernelError::Conflict`] before any retirement, and the batch commits nothing. The re-read stops with [`KernelError::Deadline`] once the plan's budget is exhausted, so it holds the writer no longer than the batch's budget allows.
    fn apply(
        &self,
        kernel: &KernelStore,
        envelope: &mut kernel::Envelope<'_>,
    ) -> Result<String, KernelError> {
        // The commit holds the writer guard, so no restore lands between this check and the retirements.
        let planned_store = envelope.commit_seq() == self.snapshot + 1
            && kernel.require_incarnation(self.incarnation).is_ok();
        if !planned_store {
            let calls: HashSet<ToolCall> =
                self.reruns.iter().map(|rerun| rerun.call.clone()).collect();
            let live = live_generations(envelope, &calls, self.budget.as_ref())?;
            for rerun in &self.reruns {
                let generations = live.get(&rerun.call).map(Vec::as_slice).unwrap_or_default();
                if predecessors(generations, rerun.successor).as_ref() != Some(&rerun.retired) {
                    return Err(KernelError::Conflict);
                }
            }
        }
        for rerun in &self.reruns {
            retire_descriptors(envelope, &rerun.retired)?;
        }
        Ok(String::new())
    }
}

/// How the re-run plan disposes of one admitted unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reconciled {
    /// Published, or replayed from its stored receipt.
    Kept,
    /// An older generation that never published: [`UnitDisposition::Superseded`].
    Superseded,
    /// An older generation whose key holds a receipt under another request digest: [`PublishError::IdentityReused`].
    Conflicting,
}

/// Reconciles the tool generations of a screened batch with the store in one preview. Each call's head is its newest generation in the batch or live in the store. A unit of an older generation is superseded unless it replays a stored receipt; a call whose head is in the batch retires its live predecessors in the batch's commit. Returns each unit's disposition and the retirements to commit as [`Reruns`], `None` when no call retires.
fn plan_reruns(
    publisher: &SourcePublisher<'_>,
    prepared: &[Prepared<'_>],
    budget: Option<&kernel::applicability::EvalBudget>,
) -> Result<(Vec<Reconciled>, Option<Reruns>), KernelError> {
    let generations: Vec<Option<(ToolCall, i64)>> = prepared
        .iter()
        .map(|unit| {
            (unit.unit.class == OccurrenceClass::RawToolSpans)
                .then(|| tool_generation(unit.identity.iter().copied()))
                .flatten()
        })
        .collect();
    let mut newest: HashMap<ToolCall, i64> = HashMap::new();
    for (call, generation) in generations.iter().flatten() {
        let entry = newest.entry(call.clone()).or_insert(*generation);
        *entry = (*entry).max(*generation);
    }
    if newest.is_empty() {
        return Ok((vec![Reconciled::Kept; prepared.len()], None));
    }
    let calls: HashSet<ToolCall> = newest.keys().cloned().collect();
    // Read before the preview, so a restore at any later point changes the incarnation the commit compares.
    let incarnation = match budget {
        Some(budget) => publisher
            .kernel
            .capture_commit_read_target_within_budget(budget)?,
        None => publisher.kernel.capture_commit_read_target()?,
    }
    .incarnation;
    let (snapshot, (live, reconciled)) =
        publisher
            .kernel
            .preview(preview_deadline(budget), |preview| {
                let live = live_generations(preview, &calls, budget)?;
                let mut reconciled = Vec::with_capacity(prepared.len());
                for (unit, generation) in prepared.iter().zip(&generations) {
                    let stale = generation.as_ref().is_some_and(|(call, generation)| {
                        let head = live.get(call).into_iter().flatten().map(|(_, live)| *live);
                        head.fold(newest[call], i64::max) > *generation
                    });
                    // A stale generation published before replays its receipt and retires nothing.
                    reconciled.push(if !stale {
                        Reconciled::Kept
                    } else {
                        match stored_receipt(
                            preview,
                            publisher.descriptor_intent(unit),
                            unit,
                            publisher,
                        ) {
                            Ok(Some(_)) => Reconciled::Kept,
                            Ok(None) => Reconciled::Superseded,
                            Err(KernelError::Conflict) => Reconciled::Conflicting,
                            Err(error) => return Err(error),
                        }
                    });
                }
                Ok((live, reconciled))
            })?;
    let reruns: Vec<Rerun> = newest
        .into_iter()
        .filter_map(|(call, successor)| {
            let generations = live.get(&call).map(Vec::as_slice).unwrap_or_default();
            let retired = predecessors(generations, successor)?;
            (!retired.is_empty()).then_some(Rerun {
                call,
                successor,
                retired,
            })
        })
        .collect();
    Ok((
        reconciled,
        (!reruns.is_empty()).then_some(Reruns {
            snapshot,
            incarnation,
            budget: budget.cloned(),
            reruns,
        }),
    ))
}

/// Retires each of `ids` that names a live descriptor.
pub(crate) fn retire_descriptors(
    envelope: &mut kernel::Envelope<'_>,
    ids: &[String],
) -> Result<String, KernelError> {
    for id in ids {
        if envelope
            .object_state(id)?
            .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
        {
            envelope.retire_observation(id)?;
        }
    }
    Ok(String::new())
}

struct Prepared<'u> {
    unit: &'u SourceUnit,
    identity: Vec<(&'u str, &'u str)>,
    occurrence_id: String,
    lineage_id: String,
    revision: i64,
    rule: ProvenanceRule,
    key: String,
    request_digest: String,
    /// The unit's `source-request.v1` digest, computed on the first receipt probe that needs it ([`Self::legacy_digest`]).
    legacy_digest: std::cell::OnceCell<String>,
    origin: String,
    project: Option<ProjectScope>,
    observed_at: i64,
}

impl Prepared<'_> {
    /// The unit's `source-request.v1` digest under `publisher`, computed once.
    fn legacy_digest(&self, publisher: &SourcePublisher<'_>) -> &str {
        self.legacy_digest
            .get_or_init(|| publisher.legacy_request_digest(self.unit))
    }

    fn evidence_object_id(&self) -> String {
        format!("srcev-object:{}", self.key)
    }

    fn evidence_id(&self) -> String {
        format!("srcev:{}", self.key)
    }

    /// The head a `repository_documents` publication expects. Its producer reads the lineage's historical head ([`kernel::KernelStore::source_lineage_head`]) and proposes exactly the head plus one, 1 for an empty lineage, so the expected head is the revision before the proposal and any gap refuses as a moved head. Every other class names no head.
    fn expected_head(&self) -> Option<i64> {
        (self.unit.class == OccurrenceClass::RepositoryDocuments && self.revision > 1)
            .then(|| self.revision - 1)
    }
}

/// The `(evidence_id, evidence object id)` a stored descriptor cites.
type CitedEvidence = Option<(String, String)>;

fn stored_descriptor(
    preview: &kernel::Envelope<'_>,
    intent: CommitIntent,
    unit: &Prepared<'_>,
    publisher: &SourcePublisher<'_>,
) -> Result<Option<(kernel::CommitReceipt, CitedEvidence)>, KernelError> {
    let Some((receipt, _)) = stored_receipt(preview, intent, unit, publisher)? else {
        return Ok(None);
    };
    let cited = preview.source_descriptor_evidence(&receipt.result)?;
    Ok(Some((receipt, cited)))
}

/// Where a unit's admission classes come from.
enum ProvenanceRule {
    Fixed(SourceClass, TaintClass),
    /// The classes are the named decision's own admission, read in the transaction that records them.
    Inherited {
        subject: String,
    },
}

impl ProvenanceRule {
    fn is_inherited(&self) -> bool {
        matches!(self, Self::Inherited { .. })
    }

    fn subject(&self) -> Option<&str> {
        match self {
            Self::Fixed(..) => None,
            Self::Inherited { subject } => Some(subject),
        }
    }

    /// `NotFound` when the inherited subject has no admission decision.
    fn resolve(
        &self,
        envelope: &kernel::Envelope<'_>,
    ) -> Result<(SourceClass, TaintClass), KernelError> {
        match self {
            Self::Fixed(source, taint) => Ok((*source, *taint)),
            Self::Inherited { subject } => envelope
                .subject_admission(subject)?
                .map(|(prior, _)| (prior.source_class, prior.taint_class))
                .ok_or(KernelError::NotFound),
        }
    }
}

/// Admission provenance follows the class and the native role: a canonical class inherits its decision's classes whatever role the unit carries, every tool string is an untrusted tool result, a commit message or repository document is untrusted repository text, user text is explicit, and other text is model inference. Only a commit carries repository provenance, so working-tree document bytes store `Sensitive`. The caller has encoded the unit, so the class's identity field is present.
fn admission_rule(unit: &SourceUnit) -> ProvenanceRule {
    match (unit.class, unit.role.as_str()) {
        (OccurrenceClass::CanonicalClaims | OccurrenceClass::PromotedMemory, _) => {
            ProvenanceRule::Inherited {
                subject: unit
                    .identity_value(unit.class.identity_fields()[0])
                    .expect("encode verified every identity field")
                    .to_owned(),
            }
        }
        (OccurrenceClass::RawToolSpans, _) => ProvenanceRule::Fixed(
            SourceClass::TrustedToolResult,
            TaintClass::ToolUntrustedOutput,
        ),
        (OccurrenceClass::GitCommits | OccurrenceClass::RepositoryDocuments, _) => {
            ProvenanceRule::Fixed(
                SourceClass::UntrustedRepoText,
                TaintClass::RepoUntrustedText,
            )
        }
        (_, "user") => ProvenanceRule::Fixed(SourceClass::ExplicitUser, TaintClass::UserExplicit),
        _ => ProvenanceRule::Fixed(SourceClass::ModelInference, TaintClass::AssistantInference),
    }
}

#[cfg(test)]
mod pins {
    /// `eval-core` has a closed dependency set and cannot name this crate.
    #[test]
    fn revision_lead_matches_the_evaluator_pin() {
        assert_eq!(super::MAX_REVISION_LEAD_MS, eval_core::MAX_REVISION_LEAD_MS);
    }
}

#[cfg(test)]
mod rerun_tests {
    use super::*;

    fn intent(key: &str) -> CommitIntent {
        CommitIntent {
            producer: "rerun-test".to_owned(),
            operation_key: key.to_owned(),
            request_digest: identity_digest(key.as_bytes()),
            actor: "test".to_owned(),
            cause: "test".to_owned(),
        }
    }

    fn generation(revision: u64) -> SourceUnit {
        let session = SessionIdentity {
            harness: Harness::Pi,
            project_id: "a".repeat(64),
            session_id: "pi-session".to_owned(),
        };
        let entry = serde_json::json!({ "type": "message", "id": format!("t{revision}"), "parentId": "a1",
            "message": { "role": "toolResult", "toolCallId": "call", "toolName": "bash", "isError": false,
                         "timestamp": revision, "content": [{ "type": "text", "text": format!("run {revision}") }] } });
        pi_units(&session, &entry).unwrap().remove(0)
    }

    fn seeded_store() -> (tempfile::TempDir, KernelStore) {
        let dir = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(dir.path()).unwrap();
        kernel
            .commit(intent("domain"), |envelope| {
                envelope.insert_domain(kernel::DomainSpec {
                    domain_id: "domain".to_owned(),
                    object_id: "domain-object".to_owned(),
                    name: "domain".to_owned(),
                    source_kind: "test".to_owned(),
                    source_id: "domain".to_owned(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(kernel::ScopeSpec {
                    scope_id: "project:a".to_owned(),
                    object_id: "project:a".to_owned(),
                    source_id: "project:a".to_owned(),
                    domain_id: "domain".to_owned(),
                    source_kind: "kernel_route".to_owned(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![kernel::ScopeTermSpec {
                        dimension: kernel::Dimension::Project.as_str().to_owned(),
                        operator: "exact".to_owned(),
                        exact_value: Some("a".repeat(64)),
                        ..kernel::ScopeTermSpec::default()
                    }],
                })?;
                Ok(String::new())
            })
            .unwrap();
        (dir, kernel)
    }

    fn publisher(kernel: &KernelStore) -> SourcePublisher<'_> {
        SourcePublisher {
            kernel,
            domain_id: "domain",
            scope_id: Some("project:a"),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Normal,
        }
    }

    fn call_of(unit: &SourceUnit) -> ToolCall {
        tool_generation(
            unit.identity
                .iter()
                .map(|(name, value)| (*name, value.as_str())),
        )
        .unwrap()
        .0
    }

    #[test]
    fn a_tool_inventory_read_stops_at_an_exhausted_budget() {
        let (_dir, kernel) = seeded_store();
        let publisher = publisher(&kernel);
        let (first, second) = (generation(100), generation(200));
        let published = publish_batch(
            &kernel,
            Vec::new(),
            &[(&publisher, &first), (&publisher, &second)],
            1_000,
            None,
        )
        .unwrap();
        let calls: HashSet<ToolCall> = [call_of(&first)].into();
        let exhausted = kernel::applicability::EvalBudget::unbounded();
        exhausted.cancel();
        let (_, read) = kernel
            .preview(Instant::now() + Duration::from_secs(30), |preview| {
                Ok((
                    live_generations(preview, &calls, Some(&exhausted)),
                    live_generations(preview, &calls, None)
                        .map(|live| live[&call_of(&first)].len()),
                ))
            })
            .unwrap();
        assert_eq!(read, (Err(KernelError::Deadline), Ok(2)));

        let reruns = Reruns {
            snapshot: 0,
            incarnation: kernel.capture_commit_read_target().unwrap().incarnation,
            budget: Some(exhausted),
            reruns: vec![Rerun {
                call: call_of(&first),
                successor: 200,
                retired: vec![published[0].object_id.clone()],
            }],
        };
        let tip = kernel.tip().unwrap();
        assert_eq!(
            kernel.commit(reruns.intent(), |envelope| reruns.apply(&kernel, envelope)),
            Err(KernelError::Deadline)
        );
        assert_eq!(
            kernel.tip().unwrap(),
            tip,
            "the refused commit writes nothing"
        );
        let (_, states) = kernel
            .object_states(&[published[0].object_id.clone()])
            .unwrap();
        assert!(
            states[0]
                .as_ref()
                .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
        );
    }

    /// `Reruns::apply` re-reads the call's live generations inside the commit. A live generation newer than the planned successor or a predecessor set differing from the plan causes `Conflict`; otherwise, the commit retires the planned predecessors.
    #[test]
    fn a_rerun_retirement_rechecks_the_call_inside_its_commit() {
        let (_dir, kernel) = seeded_store();
        let publisher = publisher(&kernel);
        let (first, second) = (generation(100), generation(200));
        // Published without a plan, both generations are live.
        let published = publish_batch(
            &kernel,
            Vec::new(),
            &[(&publisher, &first), (&publisher, &second)],
            1_000,
            None,
        )
        .unwrap();
        let (call, _) = tool_generation(
            first
                .identity
                .iter()
                .map(|(name, value)| (*name, value.as_str())),
        )
        .unwrap();
        let rerun = |successor, retired: &[&String]| Rerun {
            call: call.clone(),
            successor,
            retired: retired.iter().map(|id| (*id).clone()).collect(),
        };
        // A plan read at snapshot 0 precedes every commit here, so each commit below reads the call again.
        let incarnation = kernel.capture_commit_read_target().unwrap().incarnation;
        let from_snapshot = |snapshot, rerun| Reruns {
            snapshot,
            incarnation,
            budget: None,
            reruns: vec![rerun],
        };
        for (label, stale) in [
            ("a newer generation is live", rerun(100, &[])),
            ("a predecessor is missing from the plan", rerun(200, &[])),
        ] {
            let stale = from_snapshot(0, stale);
            assert_eq!(
                kernel.commit(stale.intent(), |envelope| stale.apply(&kernel, envelope)),
                Err(KernelError::Conflict),
                "{label}"
            );
        }
        let planned = from_snapshot(0, rerun(200, &[&published[0].object_id]));
        kernel
            .commit(planned.intent(), |envelope| {
                planned.apply(&kernel, envelope)
            })
            .unwrap();
        let (_, states) = kernel
            .object_states(&[
                published[0].object_id.clone(),
                published[1].object_id.clone(),
            ])
            .unwrap();
        assert!(
            states[0]
                .as_ref()
                .unwrap()
                .object
                .invalidated_commit_seq
                .is_some()
        );
        assert!(
            states[1]
                .as_ref()
                .unwrap()
                .object
                .invalidated_commit_seq
                .is_none()
        );

        // The operation key identifies a replacement batch by its successor and retirement set across snapshots.
        let keys: HashSet<String> = [
            from_snapshot(0, rerun(200, &[&published[0].object_id])),
            from_snapshot(9, rerun(200, &[&published[0].object_id])),
            from_snapshot(0, rerun(300, &[&published[0].object_id])),
            from_snapshot(
                0,
                rerun(200, &[&published[0].object_id, &published[1].object_id]),
            ),
            from_snapshot(0, rerun(200, &[])),
        ]
        .iter()
        .map(|reruns| reruns.intent().operation_key)
        .collect();
        assert_eq!(keys.len(), 4);
        assert_eq!(
            from_snapshot(0, rerun(200, &[&published[0].object_id]))
                .intent()
                .request_digest,
            from_snapshot(9, rerun(200, &[&published[0].object_id]))
                .intent()
                .request_digest,
            "a batch planned at another tip replays under one digest"
        );

        // A plan whose commit follows another commit reads the call again: generation 300 lands between the plan of 400 and its commit.
        let fourth = generation(400);
        let prepared = publisher.prepare(&fourth, 1_000).unwrap();
        let plan = plan_reruns(&publisher, std::slice::from_ref(&prepared), None)
            .unwrap()
            .1
            .unwrap();
        assert_eq!(plan.reruns[0].retired, [published[1].object_id.clone()]);
        let third = generation(300);
        let between = publish_batch(&kernel, Vec::new(), &[(&publisher, &third)], 1_000, None)
            .unwrap()
            .remove(0);
        let tip = kernel.tip().unwrap();
        assert_eq!(
            kernel.commit(plan.intent(), |envelope| plan.apply(&kernel, envelope)),
            Err(KernelError::Conflict),
            "a generation published after the plan's snapshot"
        );
        let mut both = vec![published[1].object_id.clone(), between.object_id.clone()];
        both.sort();
        let (_, states) = kernel.object_states(&both).unwrap();
        assert_eq!(
            kernel.tip().unwrap(),
            tip,
            "the refused commit writes nothing"
        );
        assert!(states.iter().all(|state| {
            state
                .as_ref()
                .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
        }));
        // When the store remains at the replanned snapshot, the commit retires both predecessors.
        let plan = plan_reruns(&publisher, std::slice::from_ref(&prepared), None)
            .unwrap()
            .1
            .unwrap();
        assert_eq!(plan.reruns[0].retired, both);
        kernel
            .commit(plan.intent(), |envelope| plan.apply(&kernel, envelope))
            .unwrap();
        let (_, states) = kernel.object_states(&both).unwrap();
        assert!(states.iter().all(|state| {
            state
                .as_ref()
                .is_some_and(|state| state.object.invalidated_commit_seq.is_some())
        }));

        // A restore between a plan and its commit installs another history, whose tip can return to the plan's snapshot: the commit reads the call again.
        let fifth = generation(500);
        let held = publish_batch(&kernel, Vec::new(), &[(&publisher, &fifth)], 1_000, None)
            .unwrap()
            .remove(0);
        let backups = tempfile::tempdir().unwrap();
        std::fs::set_permissions(
            backups.path(),
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        let backup = kernel
            .backup(kernel::BackupRequest {
                destination_directory: backups.path().to_path_buf(),
                deadline: Instant::now() + Duration::from_secs(30),
                capture_pin_expires_at: None,
            })
            .unwrap();
        kernel
            .commit(intent("after-backup"), |_| Ok(String::new()))
            .unwrap();
        let sixth = generation(600);
        let prepared = publisher.prepare(&sixth, 1_000).unwrap();
        let plan = plan_reruns(&publisher, std::slice::from_ref(&prepared), None)
            .unwrap()
            .1
            .unwrap();
        assert_eq!(
            plan.reruns[0].retired,
            std::slice::from_ref(&held.object_id)
        );
        kernel.restore(&backup.destination_path).unwrap();
        let restored = generation(550);
        let other = publish_batch(&kernel, Vec::new(), &[(&publisher, &restored)], 1_000, None)
            .unwrap()
            .remove(0);
        assert_eq!(kernel.tip().unwrap(), plan.snapshot, "the tip returned");
        assert_eq!(
            kernel.commit(plan.intent(), |envelope| plan.apply(&kernel, envelope)),
            Err(KernelError::Conflict),
            "a generation the restored history holds outside the plan"
        );
        let (_, states) = kernel
            .object_states(&[held.object_id, other.object_id])
            .unwrap();
        assert!(states.iter().all(|state| {
            state
                .as_ref()
                .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
        }));
    }
}
