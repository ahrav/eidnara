use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::event::{Event, EventId, EventLog, Payload};
use crate::occurrence::{EncodingRefusal, Identity, Occurrence, OccurrenceClass, encode};

const HARNESS: &str = "opencode";
const TOOL: &str = "bash";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    pub project_id: String,
    pub repository_id: String,
    pub object_format: String,
}

/// A unit the adapter must publish from a rendered message; `event_id` is
/// the join back to the event the reducer judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedUnit {
    pub event_id: EventId,
    pub revision: String,
    pub identity: Identity,
}

/// One OpenCode message fixture; `message` is the JSON `opencode_units` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMessage {
    pub event_id: EventId,
    pub session_id: String,
    pub observation_time_ms: i64,
    pub message: Value,
    pub expected: Vec<ExpectedUnit>,
}

/// A commit the shell creates in a real repository; the oid exists only then,
/// so the shell maps `oid -> valid_time_ms` and encodes the identity itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCommit {
    pub event_id: EventId,
    pub message: String,
    pub valid_time_ms: i64,
    pub observation_time_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendering {
    pub messages: Vec<RenderedMessage>,
    pub commits: Vec<RenderedCommit>,
    /// Events no adapter ingests, counted by the rule that excludes them.
    pub excluded_by_rule: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    CorrectionTargetMissing(EventId),
    CorrectionTargetIsNotAMessage(EventId),
    /// The correction's session differs from its target's. The session is an
    /// identity field, so rendering it would mint a fresh lineage.
    CorrectionTargetInOtherSession(EventId),
    /// The correction's valid time is at or before its target's. The revision
    /// is the valid time, so rendering it would reuse or precede the target.
    CorrectionDoesNotAdvance(EventId),
    /// Two base messages in one session share a `message_id`; only a
    /// `Correction` may reuse a lineage, and it says so.
    MessageIdReused(EventId),
    /// The event's unit encodes to an occurrence an earlier event already
    /// produced (a second correction at one valid time, or a second tool span
    /// with one `call_id` and valid time): two events, one identity.
    OccurrenceReused(EventId),
    /// No message in the span's session carries its `message_id`; nothing
    /// renders it and no rule excludes it, so it would vanish from accounting.
    ToolSpanParentMissing(EventId),
    /// A message and its tool parts are one fixture observed once; a span
    /// observed at another time than its parent cannot be rendered faithfully.
    ToolSpanObservationDiffers(EventId),
    /// `RenderConfig` binds one `repository_id`; a second repository entity
    /// would render under the first's identity.
    SecondRepository(EventId),
    UnknownRole {
        event_id: EventId,
        role: String,
    },
    /// An assistant turn at valid time zero has no earlier `created`.
    NoEarlierCreated(EventId),
    Encoding {
        event_id: EventId,
        refusal: EncodingRefusal,
    },
}

debug_display!(RenderError);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountingError {
    Missing(BTreeSet<String>),
    Unexpected(BTreeSet<String>),
    PublishedAndRefused(BTreeSet<String>),
}

debug_display!(AccountingError);

/// `expected == published + refused` as sets, so no unit is lost silently.
pub fn check_accounting(
    expected: &BTreeSet<String>,
    published: &BTreeSet<String>,
    refused: &BTreeSet<String>,
) -> Result<(), AccountingError> {
    let both: BTreeSet<String> = published.intersection(refused).cloned().collect();
    if !both.is_empty() {
        return Err(AccountingError::PublishedAndRefused(both));
    }
    let outcomes: BTreeSet<String> = published.union(refused).cloned().collect();
    let missing: BTreeSet<String> = expected.difference(&outcomes).cloned().collect();
    if !missing.is_empty() {
        return Err(AccountingError::Missing(missing));
    }
    let unexpected: BTreeSet<String> = outcomes.difference(expected).cloned().collect();
    if !unexpected.is_empty() {
        return Err(AccountingError::Unexpected(unexpected));
    }
    Ok(())
}

pub fn git_identity(config: &RenderConfig, oid: &str) -> Result<Identity, EncodingRefusal> {
    let identity = [
        ("repository_id", config.repository_id.as_str()),
        ("object_format", config.object_format.as_str()),
        ("oid", oid),
    ];
    encode(&Occurrence {
        class: OccurrenceClass::GitCommits.code(),
        identity: &identity,
        revision: "1",
        representation: "commit_message",
        span: None,
    })
}

/// The message identity a rendered event carries: a correction reuses its
/// target's `message_id` and role at the correction's own valid time.
struct Message<'a> {
    event: &'a Event,
    message_id: &'a str,
    role: &'a str,
}

impl Message<'_> {
    fn unit(
        event: &Event,
        class: OccurrenceClass,
        identity: &[(&str, &str)],
        revision: &str,
        representation: &str,
    ) -> Result<ExpectedUnit, RenderError> {
        let identity = encode(&Occurrence {
            class: class.code(),
            identity,
            revision,
            representation,
            span: None,
        })
        .map_err(|refusal| RenderError::Encoding {
            event_id: event.id.clone(),
            refusal,
        })?;
        Ok(ExpectedUnit {
            event_id: event.id.clone(),
            revision: revision.to_string(),
            identity,
        })
    }

    fn text_unit(&self, config: &RenderConfig) -> Result<ExpectedUnit, RenderError> {
        let identity = [
            ("project_id", config.project_id.as_str()),
            ("harness", HARNESS),
            ("session_id", self.event.entity_id.as_str()),
            ("message_id", self.message_id),
            ("block_index", "0"),
        ];
        let revision = self.event.valid_time_ms.to_string();
        Self::unit(
            self.event,
            OccurrenceClass::Messages,
            &identity,
            &revision,
            "text",
        )
    }

    /// One completed tool part per tool span; the span's valid time is its
    /// `result_revision` and revision.
    fn tool_part(
        &self,
        config: &RenderConfig,
        span: &Event,
        call_id: &str,
        output: &str,
    ) -> Result<(Value, ExpectedUnit), RenderError> {
        let end = span.valid_time_ms.to_string();
        let identity = [
            ("project_id", config.project_id.as_str()),
            ("harness", HARNESS),
            ("session_id", self.event.entity_id.as_str()),
            ("parent_message_id", self.message_id),
            ("tool_call_id", call_id),
            ("result_revision", end.as_str()),
            ("block_index", "0"),
        ];
        let part = json!({
            "type": "tool", "callID": call_id, "tool": TOOL,
            "state": {"status": "completed", "input": {}, "output": output,
                      "time": {"start": span.valid_time_ms, "end": span.valid_time_ms}},
        });
        let unit = Self::unit(
            span,
            OccurrenceClass::RawToolSpans,
            &identity,
            &end,
            "tool_output",
        )?;
        Ok((part, unit))
    }

    /// Valid time is the revision: `created` for a user turn, `completed` for
    /// an assistant turn, whose `created` sits one millisecond earlier so the
    /// adapter's precedence is exercised rather than assumed.
    fn rendered(
        &self,
        parts: Vec<Value>,
        expected: Vec<ExpectedUnit>,
        occurrences: &mut BTreeSet<String>,
    ) -> Result<RenderedMessage, RenderError> {
        for unit in &expected {
            if !occurrences.insert(unit.identity.occurrence_id.clone()) {
                return Err(RenderError::OccurrenceReused(unit.event_id.clone()));
            }
        }
        let valid = self.event.valid_time_ms;
        let time = match self.role {
            "user" => json!({"created": valid}),
            "assistant" => {
                let created = valid
                    .checked_sub(1)
                    .filter(|created| *created >= 0)
                    .ok_or_else(|| RenderError::NoEarlierCreated(self.event.id.clone()))?;
                json!({"created": created, "completed": valid})
            }
            other => {
                return Err(RenderError::UnknownRole {
                    event_id: self.event.id.clone(),
                    role: other.to_string(),
                });
            }
        };
        Ok(RenderedMessage {
            event_id: self.event.id.clone(),
            session_id: self.event.entity_id.clone(),
            observation_time_ms: self.event.observation_time_ms,
            message: json!({
                "info": {"id": self.message_id, "sessionID": self.event.entity_id, "role": self.role, "time": time},
                "parts": parts,
            }),
            expected,
        })
    }
}

/// Renders every message, tool span, and correction as OpenCode session
/// fixtures with explicit times, and every commit as a repository fixture.
pub fn render(log: &EventLog, config: &RenderConfig) -> Result<Rendering, RenderError> {
    let mut rendering = Rendering {
        messages: Vec::new(),
        commits: Vec::new(),
        excluded_by_rule: BTreeMap::new(),
    };
    let mut occurrences = BTreeSet::new();
    let mut repository: Option<&str> = None;
    let messages_named = |entity_id: &str, id: &str| {
        log.events
            .iter()
            .filter(|e| {
                e.entity_id == entity_id
                    && matches!(&e.payload, Payload::Message { message_id, .. } if message_id == id)
            })
            .count()
    };
    for event in &log.events {
        match &event.payload {
            Payload::Message {
                message_id,
                role,
                text,
                ..
            } => {
                if messages_named(&event.entity_id, message_id) > 1 {
                    return Err(RenderError::MessageIdReused(event.id.clone()));
                }
                let m = Message {
                    event,
                    message_id,
                    role,
                };
                let mut parts = vec![json!({"type": "text", "text": text})];
                let mut expected = vec![m.text_unit(config)?];
                let spans = log.events.iter().filter_map(|span| match &span.payload {
                    Payload::ToolSpan {
                        message_id: parent,
                        call_id,
                        output,
                    } if span.entity_id == event.entity_id && parent == message_id => {
                        Some((span, call_id, output))
                    }
                    _ => None,
                });
                for (span, call_id, output) in spans {
                    if span.observation_time_ms != event.observation_time_ms {
                        return Err(RenderError::ToolSpanObservationDiffers(span.id.clone()));
                    }
                    let (part, unit) = m.tool_part(config, span, call_id, output)?;
                    parts.push(part);
                    expected.push(unit);
                }
                rendering
                    .messages
                    .push(m.rendered(parts, expected, &mut occurrences)?);
            }
            Payload::Correction { target, text } => {
                let original = log
                    .events
                    .iter()
                    .find(|e| e.id == *target)
                    .ok_or_else(|| RenderError::CorrectionTargetMissing(target.clone()))?;
                let Payload::Message {
                    message_id, role, ..
                } = &original.payload
                else {
                    return Err(RenderError::CorrectionTargetIsNotAMessage(target.clone()));
                };
                if original.entity_id != event.entity_id {
                    return Err(RenderError::CorrectionTargetInOtherSession(target.clone()));
                }
                if event.valid_time_ms <= original.valid_time_ms {
                    return Err(RenderError::CorrectionDoesNotAdvance(target.clone()));
                }
                let m = Message {
                    event,
                    message_id,
                    role,
                };
                let expected = vec![m.text_unit(config)?];
                let parts = vec![json!({"type": "text", "text": text})];
                rendering
                    .messages
                    .push(m.rendered(parts, expected, &mut occurrences)?);
            }
            Payload::Commit { message, .. } => {
                if *repository.get_or_insert(&event.entity_id) != event.entity_id {
                    return Err(RenderError::SecondRepository(event.id.clone()));
                }
                rendering.commits.push(RenderedCommit {
                    event_id: event.id.clone(),
                    message: message.clone(),
                    valid_time_ms: event.valid_time_ms,
                    observation_time_ms: event.observation_time_ms,
                })
            }
            Payload::ToolSpan { message_id, .. } => {
                if messages_named(&event.entity_id, message_id) == 0 {
                    return Err(RenderError::ToolSpanParentMissing(event.id.clone()));
                }
            }
            Payload::Rename { .. } => {
                *rendering
                    .excluded_by_rule
                    .entry("rename_is_commit_metadata".to_string())
                    .or_default() += 1
            }
            Payload::Invalidation { .. } => {
                *rendering
                    .excluded_by_rule
                    .entry("invalidation_has_no_adapter".to_string())
                    .or_default() += 1
            }
        }
    }
    Ok(rendering)
}
