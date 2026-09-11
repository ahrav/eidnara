//! Turns native OpenCode and Pi session records into kernel source descriptors: one occurrence per ordered text block of a message, and one per tool output or error string, each bound to the native identity the harness gave it.
//!
//! Identity is native or refused. A message occurrence names the project, harness, durable session, native message, and block position; a raw tool occurrence names the parent message, tool call, result revision, and output block. A payload hash never stands in for a missing identity. An OpenCode record names its session and is refused when it names another; Pi entries carry no session, so the caller's binding of the session file is the session identity. Message parts that are not text (reasoning, steps, files, patches, images) are not text sources and contribute nothing; a tool result whose content is not text is refused, because the tool string is the source and a non-text one has no disposition here. Tool strings are retained exactly as the codec received them and are never joined; a tool result is never part of a message's text, and its class creates no dense work downstream.
//!
//! Publication goes through the kernel's shared source operation: the block is retained as an exact artifact (which runs the bounded secret scan and refuses rewritten or unscannable bytes), then one commit publishes the descriptor with its canonical revision and records the admission. Both steps carry receipt keys derived from the canonical occurrence identity, revision, and representation, and a request digest over the text and the publishing context, so a replay returns the stored receipt instead of a second row, the same unit under another scope or with other text is a conflict, and a newer revision of the same lineage invalidates its predecessor in the same commit. A descriptor refusal after the artifact was retained retires the evidence in a compensating commit, so no refused block stays a live source.

use kernel::source_identity::{
    HARNESSES, Occurrence, OccurrenceClass, encode_preserving_span, identity_digest,
};

const _: () = assert!(
    HARNESSES.len() == 2,
    "both harness spellings are the kernel's"
);
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactErrorKind, ArtifactIngestRequest, CommitIntent,
    EventKind, KernelError, KernelStore, ProviderEgress, Sensitivity, SourceClass,
    SourceDescriptorError, SourceDescriptorPolicy, SourceDescriptorRequest, TaintClass,
};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// The route-bound identity every unit of a session is published under. `project_id` is the bound project's digest, the scope term's exact value.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

impl Representation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::ToolOutput => "tool_output",
            Self::ToolError => "tool_error",
        }
    }
}

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
            .finish()
    }
}

impl SourceUnit {
    fn value(&self, name: &str) -> &str {
        self.identity
            .iter()
            .find(|(field, _)| *field == name)
            .map_or("", |(_, value)| value.as_str())
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
    value
        .get(name)
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
            identity: vec![
                ("project_id", self.project_id.clone()),
                ("harness", self.harness.as_str().to_owned()),
                ("session_id", self.session_id.clone()),
                ("message_id", message_id.to_owned()),
                ("block_index", block_index.to_string()),
            ],
            revision: revision.to_owned(),
            representation: Representation::Text,
            text: text.to_owned(),
            role: role.to_owned(),
        }
    }

    fn tool_unit(&self, tool: &ToolBlock<'_>, block_index: usize, text: &str) -> SourceUnit {
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
        }
    }
}

struct ToolBlock<'a> {
    parent_message_id: &'a str,
    tool_call_id: &'a str,
    result_revision: String,
    is_error: bool,
}

/// Units of one OpenCode `MessageV2` record: every `text` part in position order (its block index is its part position), and every `tool` part whose state settled, as the exact `state.output` or `state.error` string (its output block index is 0, the one string). Ignored text and parts of other kinds contribute nothing.
///
/// # Errors
///
/// Refuses a record without `info.id`, `info.sessionID`, `info.role`, or `info.time`; a record from another session; a settled tool part without `callID` or `state.time.end`; and a settled tool part whose output is not a string.
pub fn opencode_units(
    session: &SessionIdentity,
    message: &Value,
) -> Result<Vec<SourceUnit>, AdapterRefusal> {
    let info = message
        .get("info")
        .filter(|info| info.is_object())
        .ok_or(AdapterRefusal::NotAnObject)?;
    let message_id = field(info, "id")?;
    if field(info, "sessionID")? != session.session_id {
        return Err(AdapterRefusal::SessionMismatch);
    }
    let role = field(info, "role")?;
    let time = info
        .get("time")
        .ok_or(AdapterRefusal::MissingIdentity("time"))?;
    let message_revision = revision(
        time.get("completed").or_else(|| time.get("created")),
        "time",
    )?;
    let mut units = Vec::new();
    for (index, part) in message
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        match part.get("type").and_then(Value::as_str) {
            // An ignored text part is not conversation text.
            Some("text") if part.get("ignored").and_then(Value::as_bool) == Some(true) => {}
            Some("text") => {
                let text = part
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or(AdapterRefusal::UnsupportedShape("text part"))?;
                units.push(session.message_unit(message_id, index, &message_revision, role, text));
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
                // One settled tool part carries one output string: its only output block.
                units.push(session.tool_unit(&tool, 0, output));
            }
            _ => {}
        }
    }
    Ok(units)
}

/// Units of one Pi session entry: for a user or assistant message, every text content item in position order (a bare string is one block); for a tool result, every text content item as its own block under the call it answers. Entries of other types contribute nothing.
///
/// # Errors
///
/// Refuses a message entry without `id`, `message.role`, or a numeric `message.timestamp`; a tool result without `toolCallId` or `parentId`; and a tool result content item that is not text.
pub fn pi_units(
    session: &SessionIdentity,
    entry: &Value,
) -> Result<Vec<SourceUnit>, AdapterRefusal> {
    if !entry.is_object() {
        return Err(AdapterRefusal::NotAnObject);
    }
    if entry.get("type").and_then(Value::as_str) != Some("message") {
        return Ok(Vec::new());
    }
    let entry_id = field(entry, "id")?;
    let message = entry
        .get("message")
        .filter(|message| message.is_object())
        .ok_or(AdapterRefusal::MissingIdentity("message"))?;
    let role = field(message, "role")?;
    let stamp = revision(message.get("timestamp"), "timestamp")?;
    let mut units = Vec::new();
    if role == "toolResult" {
        let tool = ToolBlock {
            parent_message_id: field(entry, "parentId")?,
            tool_call_id: field(message, "toolCallId")?,
            result_revision: stamp,
            is_error: message
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        for (index, item) in content_items(message)?.iter().enumerate() {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .filter(|_| item.get("type").and_then(Value::as_str) == Some("text"))
                .ok_or(AdapterRefusal::UnsupportedShape("tool result item"))?;
            units.push(session.tool_unit(&tool, index, text));
        }
        return Ok(units);
    }
    match message.get("content") {
        Some(Value::String(text)) => {
            units.push(session.message_unit(entry_id, 0, &stamp, role, text))
        }
        _ => {
            for (index, item) in content_items(message)?.iter().enumerate() {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    let text = item
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or(AdapterRefusal::UnsupportedShape("text item"))?;
                    units.push(session.message_unit(entry_id, index, &stamp, role, text));
                }
            }
        }
    }
    Ok(units)
}

fn content_items(message: &Value) -> Result<&Vec<Value>, AdapterRefusal> {
    message
        .get("content")
        .and_then(Value::as_array)
        .ok_or(AdapterRefusal::UnsupportedShape("content"))
}

/// The receipt outcome of publishing one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub object_id: String,
    pub occurrence_id: String,
    /// The predecessor revision this publication invalidated in the same commit.
    pub replaced_object_id: Option<String>,
    /// The kernel answered from a stored receipt: the same unit was published before.
    pub replayed: bool,
    /// The admission classes the unit was recorded under.
    pub provenance: (SourceClass, TaintClass),
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The exact artifact was refused: the bytes were rewritten by the scanner, could not be scanned, exceed the store's bounds, or differ from bytes already retained under the same identity and revision. Nothing new was retained.
    #[error("the block's bytes were refused: {0:?}")]
    Artifact(ArtifactErrorKind),
    #[error("the descriptor was refused: {0}")]
    Descriptor(SourceDescriptorError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

const PRODUCER: &str = "eidnara-daemon/harness-sources";

/// Publishes units into one kernel under one domain and scope. `egress` is the caller's provider policy for the retained evidence.
pub struct SourcePublisher<'a> {
    pub kernel: &'a KernelStore,
    pub domain_id: &'a str,
    pub scope_id: &'a str,
    pub egress: ProviderEgress,
    pub sensitivity: Sensitivity,
}

impl SourcePublisher<'_> {
    /// Retains the unit's text as an exact artifact, then publishes its descriptor and records its admission in one commit. Both receipts are keyed by the unit's identity and revision, so publishing the same unit again replays them.
    ///
    /// # Errors
    ///
    /// Returns [`PublishError::Artifact`] when the bytes are refused before anything is retained, [`PublishError::Descriptor`] when the kernel refuses the descriptor (a stale revision, a colliding tuple, a mismatched domain), and [`PublishError::Kernel`] for store failures.
    pub fn publish(&self, unit: &SourceUnit, observed_at: i64) -> Result<Published, PublishError> {
        let identity: Vec<(&str, &str)> = unit
            .identity
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let occurrence = Occurrence {
            class: unit.class.code(),
            identity: &identity,
            revision: &unit.revision,
            representation: unit.representation.as_str(),
            span: None,
        };
        // The tuple is judged before any byte is retained, so an oversized or malformed identity refuses without partial publication.
        let encoded = encode_preserving_span(&occurrence)
            .map_err(|refusal| PublishError::Descriptor(SourceDescriptorError::from(refusal)))?;
        let key = identity_digest(
            format!(
                "{}\u{1f}{}\u{1f}{}",
                encoded.occurrence_id,
                encoded.revision,
                unit.representation.as_str()
            )
            .as_bytes(),
        );
        let request_digest = self.request_digest(unit);
        // Harness text has no affirmative provenance, so the store folds an asserted `Normal` up to `Sensitive`; the descriptor records the same class.
        let sensitivity = match self.sensitivity {
            Sensitivity::Normal => Sensitivity::Sensitive,
            other => other,
        };
        let harness = unit.value("harness").to_owned();
        let handle = self
            .kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: self.intent(&format!("source-evidence:{key}"), &request_digest, &harness),
                payload: unit.text.as_bytes().to_vec(),
                evidence_id: format!("srcev:{key}"),
                object_id: format!("srcev-object:{key}"),
                object_kind: "evidence".to_owned(),
                domain_id: self.domain_id.to_owned(),
                source_kind: unit.class.code().to_owned(),
                source_id: format!("{harness}:{key}"),
                source_revision: encoded.revision,
                media_type: "text/plain".to_owned(),
                retention_class: "canonical".to_owned(),
                retain_until: None,
                asserted_sensitivity: sensitivity,
                provider_egress: self.egress,
                provenance: None,
            })
            .map_err(|error| PublishError::Artifact(error.kind()))?;
        let provenance = provenance(unit);
        let mut outcome = None;
        let receipt = self.kernel.commit(
            self.intent(
                &format!("source-descriptor:{key}"),
                &request_digest,
                &harness,
            ),
            |envelope| {
                let published = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: occurrence.clone(),
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: self.domain_id,
                        scope_id: Some(self.scope_id),
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: &unit.text,
                        sensitivity,
                        observed_at,
                    })
                    .map_err(|refusal| {
                        outcome = Some(Err(refusal));
                        KernelError::InvalidInput
                    })?;
                envelope.record_admission(AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(published.object_id.clone()),
                    source_class: Some(provenance.0),
                    taint_class: Some(provenance.1),
                    event: AdmissionEvent {
                        kind: EventKind::Other,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: Some(handle.evidence_id.clone()),
                        reason: "harness source publication".to_owned(),
                    },
                })?;
                let result = published.object_id.clone();
                outcome = Some(Ok(published));
                Ok(result)
            },
        );
        match (receipt, outcome) {
            (Ok(receipt), Some(Ok(published))) => Ok(Published {
                object_id: published.object_id,
                occurrence_id: published.occurrence_id,
                replaced_object_id: published.replaced_object_id,
                replayed: receipt.replayed,
                provenance,
            }),
            // A replayed receipt ran no operation; its result is the object id the first publication returned.
            (Ok(receipt), _) => Ok(Published {
                object_id: receipt.result,
                occurrence_id: encoded.occurrence_id,
                replaced_object_id: None,
                replayed: true,
                provenance,
            }),
            (Err(error), outcome) => {
                // The evidence was retained for a descriptor the kernel refused; retiring it leaves no live source behind.
                let object_id = format!("srcev-object:{key}");
                let _ = self.kernel.commit(
                    self.intent(
                        &format!("source-evidence-retire:{key}"),
                        &request_digest,
                        &harness,
                    ),
                    |envelope| {
                        envelope.retire_observation(&object_id)?;
                        Ok(String::new())
                    },
                );
                Err(match outcome {
                    Some(Err(refusal)) => PublishError::Descriptor(refusal),
                    _ => PublishError::Kernel(error),
                })
            }
        }
    }

    fn intent(&self, operation: &str, request_digest: &str, harness: &str) -> CommitIntent {
        CommitIntent {
            producer: PRODUCER.to_owned(),
            operation_key: operation.to_owned(),
            request_digest: request_digest.to_owned(),
            actor: harness.to_owned(),
            cause: "harness source publication".to_owned(),
        }
    }

    /// The request digest covers the text and the publishing context, so the same identity republished with other bytes, into another scope or domain, or under another class is a conflict rather than a replay.
    fn request_digest(&self, unit: &SourceUnit) -> String {
        let mut bytes = Vec::new();
        for part in [
            unit.text.as_str(),
            self.domain_id,
            self.scope_id,
            unit.role.as_str(),
            self.sensitivity.as_str(),
        ] {
            bytes.extend_from_slice(part.as_bytes());
            bytes.push(0x1f);
        }
        identity_digest(&bytes)
    }
}

/// Admission provenance follows the native role: user text is explicit, assistant text is model inference, and every tool string is an untrusted tool result.
fn provenance(unit: &SourceUnit) -> (SourceClass, TaintClass) {
    match (unit.class, unit.role.as_str()) {
        (OccurrenceClass::RawToolSpans, _) => (
            SourceClass::TrustedToolResult,
            TaintClass::ToolUntrustedOutput,
        ),
        (_, "user") => (SourceClass::ExplicitUser, TaintClass::UserExplicit),
        _ => (SourceClass::ModelInference, TaintClass::AssistantInference),
    }
}
