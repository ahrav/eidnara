//! Turns native OpenCode and Pi session records into kernel source descriptors: one occurrence per ordered text block of a message, and one per tool output or error string, each bound to the native identity the harness gave it.
//!
//! Identity is native or refused. A message occurrence names the project, harness, durable session, native message, and block position; a raw tool occurrence names the parent message, tool call, result revision, and output block. A payload hash never stands in for a missing identity. An OpenCode record names its session and is refused when it names another; Pi entries carry no session, so the caller's binding of the session file is the session identity. Message parts that are not text (reasoning, steps, files, patches, images) are not text sources and contribute nothing; a tool result whose content is not text is refused, because the tool string is the source and a non-text one has no disposition here. Tool strings are retained exactly as the codec received them and are never joined; a tool result is never part of a message's text, and its class creates no dense work downstream.
//!
//! Publication retains each block's exact bytes, then commits its descriptor and admission. Receipt keys make replays idempotent; changed bytes or publishing context conflict. A newer revision invalidates its predecessor atomically. Permanent descriptor refusals attempt to retire retained evidence. Kernel failures, including unmet preconditions such as a missing scope, preserve it; callers decide whether the failure permits retry.

use kernel::source_identity::{
    HARNESSES, Occurrence, OccurrenceClass, encode_preserving_span, identity_digest,
};

const _: () = assert!(
    HARNESSES.len() == 2,
    "both harness spellings are the kernel's"
);
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactErrorKind, ArtifactIngestRequest, CommitIntent,
    EventKind, KernelError, KernelStore, ProjectScope, ProviderEgress, Sensitivity, SourceClass,
    SourceDescriptorError, SourceDescriptorPolicy, SourceDescriptorRequest, TaintClass,
};
use serde_json::Value;
use std::time::{Duration, Instant};

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
    DecisionSummary,
    Rationale,
    Summary,
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
        }
    }
}

/// Units with this role take their admission classes from the source decision's own admission at publication instead of a role mapping.
pub const CANONICAL_ROLE: &str = "canonical";

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
    fn identity_value(&self, name: &str) -> Option<&str> {
        self.identity
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| value.as_str())
    }

    /// The actor a unit's commits and evidence are recorded under: the harness that produced it, or the canonical role for a unit derived from a kernel decision.
    fn origin(&self) -> &str {
        self.identity_value("harness").unwrap_or(CANONICAL_ROLE)
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

/// Units of one OpenCode `MessageV2` record: user text at `time.created`, assistant text at `time.completed`, and settled tool parts at `state.time.end`. Text block indices are part positions; a tool's output block index is 0, its one exact output or error string. Ignored text and other part kinds contribute nothing.
///
/// Assistant text without a completion timestamp is deferred. A valid completion timestamp makes non-ignored text eligible even when `info.error` is present. Settled tools contribute independently. The adapter uses no separate edit revision for user text or completed assistant text; changing bytes at the same native identity and timestamp conflicts at publication.
///
/// # Errors
///
/// The OpenCode adapter refuses a non-OpenCode binding, non-object `info`, missing identities, roles other than `user` or `assistant`, a different session, missing or invalid timestamps, non-array `parts`, non-string selected text, tool parts without `state`, and settled tools without `callID`, `state.time.end`, or string output.
pub fn opencode_units(
    session: &SessionIdentity,
    message: &Value,
) -> Result<Vec<SourceUnit>, AdapterRefusal> {
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
    let mut units = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        match part.get("type").and_then(Value::as_str) {
            // An ignored text part is not conversation text.
            Some("text") if part.get("ignored").and_then(Value::as_bool) == Some(true) => {}
            Some("text") if role == "assistant" && completed.is_none() => {}
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

/// Units of one Pi session entry: for a user or assistant message, every text content item in position order (a bare string is one block); for a tool result, every text content item as its own block under the call it answers. Entries of other types contribute nothing. `tool_call_id` is the entry's `toolCallId` verbatim, including a provider's composite `call|item` spelling; the wire codec's canonical call id is a different value and is not an identity here.
///
/// # Errors
///
/// The Pi adapter refuses a non-Pi binding, non-object entries or messages, missing identities or timestamps, invalid timestamps, unsupported roles, malformed content containers or text items, and tool results without a call, parent, or boolean `isError`, or with non-text content.
pub fn pi_units(
    session: &SessionIdentity,
    entry: &Value,
) -> Result<Vec<SourceUnit>, AdapterRefusal> {
    if session.harness != Harness::Pi {
        return Err(AdapterRefusal::UnsupportedShape("harness"));
    }
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
    if !matches!(role, "user" | "assistant" | "toolResult") {
        return Err(AdapterRefusal::UnsupportedShape("message.role"));
    }
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
                .ok_or(AdapterRefusal::UnsupportedShape("message.isError"))?,
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
    /// The evidence object retaining the unit's exact bytes.
    pub evidence_object_id: String,
    /// The predecessor revision this publication invalidated in the same commit.
    pub replaced_object_id: Option<String>,
    /// The kernel answered from a stored receipt: the same unit was published before.
    pub replayed: bool,
    /// The admission classes the unit was recorded under.
    pub provenance: (SourceClass, TaintClass),
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
    /// The descriptor was refused: a malformed or oversized identity before any byte was retained (`evidence` is `None`), or a stale revision, a colliding tuple, or a mismatched domain after the evidence was retained.
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

const PRODUCER: &str = "eidnara-daemon/harness-sources";
const CAUSE: &str = "source publication";

/// How far a native millisecond timestamp may lead the caller's observation and still be accepted as a revision.
pub const MAX_REVISION_LEAD_MS: i64 = 60 * 60 * 1_000;

/// How long `publish` waits for a kernel reader to consult the stored receipt before offering bytes to the store.
const RECEIPT_WAIT: Duration = Duration::from_secs(30);

/// `egress` controls the provider policy for retained evidence.
pub struct SourcePublisher<'a> {
    pub kernel: &'a KernelStore,
    pub domain_id: &'a str,
    /// `None` publishes unscoped rows that no project route serves.
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
        let encoded =
            encode_preserving_span(&occurrence).map_err(|refusal| PublishError::Descriptor {
                refusal: SourceDescriptorError::from(refusal),
                evidence: None,
            })?;
        let rule = provenance(unit);
        // A native timestamp may not lead the observation; a canonical revision is the kernel's own counter and is not judged against the clock.
        if !rule.is_inherited()
            && encoded.revision > observed_at.saturating_add(MAX_REVISION_LEAD_MS)
        {
            return Err(PublishError::RevisionAhead {
                revision: encoded.revision,
                observed_at,
            });
        }
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
        let origin = unit.origin().to_owned();
        let evidence_object_id = format!("srcev-object:{key}");
        // Only a unit that names a project has a scope term to check; a canonical unit's scope is the decision's own.
        let project = match unit.identity_value("project_id") {
            Some(project_id) => {
                Some(
                    ProjectScope::new(project_id).map_err(|error| PublishError::Kernel {
                        error,
                        evidence: None,
                    })?,
                )
            }
            None => None,
        };
        let check_scope = |envelope: &kernel::Envelope<'_>| {
            if let (Some(project), Some(scope_id)) = (&project, self.scope_id)
                && let Some(terms) = envelope.scope_terms(scope_id)?
                && !project.names_project(Some(&terms))
            {
                return Err(KernelError::NotFound);
            }
            Ok(())
        };
        // A unit published before answers from its descriptor receipt without offering its bytes to the store again; a receipt under another request is the conflict the digest exists to catch.
        let descriptor_intent = self.intent(
            &format!("source-descriptor:{key}"),
            &request_digest,
            &origin,
        );
        let (_, (stored, provenance)) = self
            .kernel
            .preview(Instant::now() + RECEIPT_WAIT, |preview| {
                check_scope(preview)?;
                Ok((
                    preview.stored_receipt(descriptor_intent.clone())?,
                    rule.resolve(preview)?,
                ))
            })
            .map_err(|error| match error {
                KernelError::Conflict => PublishError::IdentityReused,
                KernelError::NotFound if rule.is_inherited() => {
                    PublishError::UnadmittedSource { evidence: None }
                }
                error => PublishError::Kernel {
                    error,
                    evidence: None,
                },
            })?;
        if let Some(receipt) = stored {
            return Ok(Published {
                object_id: receipt.result,
                occurrence_id: encoded.occurrence_id,
                evidence_object_id,
                replaced_object_id: None,
                replayed: true,
                provenance,
            });
        }
        let handle = self
            .kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: self.intent(&format!("source-evidence:{key}"), &request_digest, &origin),
                payload: unit.text.as_bytes().to_vec(),
                evidence_id: format!("srcev:{key}"),
                object_id: evidence_object_id.clone(),
                object_kind: "evidence".to_owned(),
                domain_id: self.domain_id.to_owned(),
                source_kind: unit.class.code().to_owned(),
                source_id: format!("{origin}:{key}"),
                source_revision: encoded.revision,
                media_type: "text/plain".to_owned(),
                retention_class: "canonical".to_owned(),
                retain_until: None,
                asserted_sensitivity: self.sensitivity,
                provider_egress: self.egress,
                provenance: None,
            })
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
        let mut outcome = None;
        let mut committed_provenance = provenance;
        let receipt = self.kernel.commit(descriptor_intent, |envelope| {
            check_scope(envelope)?;
            // The classes recorded are the ones the commit itself observes, so an admission change since the preview is not carried forward.
            let (source_class, taint_class) = rule.resolve(envelope)?;
            committed_provenance = (source_class, taint_class);
            let published = envelope
                .publish_source_descriptor(&SourceDescriptorRequest {
                    occurrence: occurrence.clone(),
                    source_policy: SourceDescriptorPolicy::Native,
                    domain_id: self.domain_id,
                    scope_id: self.scope_id,
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
                source_class: Some(source_class),
                taint_class: Some(taint_class),
                event: AdmissionEvent {
                    kind: EventKind::Other,
                    trigger_object_id: rule.subject().map(str::to_owned),
                    approval_object_id: None,
                    evidence_id: Some(handle.evidence_id.clone()),
                    reason: CAUSE.to_owned(),
                },
            })?;
            let result = published.object_id.clone();
            outcome = Some(Ok(published));
            Ok(result)
        });
        match (receipt, outcome) {
            (Ok(receipt), Some(Ok(published))) => Ok(Published {
                object_id: published.object_id,
                occurrence_id: published.occurrence_id,
                evidence_object_id,
                replaced_object_id: published.replaced_object_id,
                replayed: receipt.replayed,
                provenance: committed_provenance,
            }),
            // A receipt committed between the preview and this commit replays here; it ran no operation, and its result is the object id the first publication returned.
            (Ok(receipt), _) => Ok(Published {
                object_id: receipt.result,
                occurrence_id: encoded.occurrence_id,
                evidence_object_id,
                replaced_object_id: None,
                replayed: true,
                provenance,
            }),
            (Err(error), outcome) => {
                let error = match outcome {
                    Some(Err(SourceDescriptorError::Kernel(error))) => error,
                    None if error == KernelError::NotFound && rule.is_inherited() => {
                        return Err(PublishError::UnadmittedSource {
                            evidence: Some(RetainedEvidence {
                                object_id: evidence_object_id,
                                retired: None,
                            }),
                        });
                    }
                    Some(Err(refusal)) => {
                        let evidence = Some(self.retire_evidence(&key, &request_digest, &origin));
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

    /// The request digest binds text, domain, scope, role, sensitivity, and provider-egress policy. Changing any of these under the same receipt key conflicts rather than replaying.
    fn request_digest(&self, unit: &SourceUnit) -> String {
        let mut bytes = Vec::new();
        for part in [
            unit.text.as_str(),
            self.domain_id,
            self.scope_id.unwrap_or_default(),
            unit.role.as_str(),
            self.sensitivity.as_str(),
        ] {
            bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
            bytes.extend_from_slice(part.as_bytes());
        }
        bytes.push(u8::from(self.scope_id.is_some()));
        bytes.push(match self.egress {
            ProviderEgress::RemoteAllowed => 0,
            ProviderEgress::LocalOnly => 1,
        });
        identity_digest(&bytes)
    }
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

/// Admission provenance follows the class and the native role: a canonical class inherits its decision's classes whatever role the unit carries, every tool string is an untrusted tool result, user text is explicit, and other text is model inference. The caller has encoded the unit, so the class's identity field is present.
fn provenance(unit: &SourceUnit) -> ProvenanceRule {
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
        (_, "user") => ProvenanceRule::Fixed(SourceClass::ExplicitUser, TaintClass::UserExplicit),
        _ => ProvenanceRule::Fixed(SourceClass::ModelInference, TaintClass::AssistantInference),
    }
}
