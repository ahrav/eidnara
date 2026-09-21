use std::collections::BTreeMap;

use context_core::canonical_json::protocol_digest;
use serde::{Deserialize, Serialize};

pub const EVENT_SCHEMA_VERSION: &str = "eval-events/v1";
pub const LINEARIZATION_RULE_VERSION: &str = "eval-linearization/v1";
pub const LOG_DIGEST_PROTOCOL: &str = "eval-event-log/v1";

/// Valid time may lead observation time by at most this; `harness_sources::publish` pins the same value.
pub const MAX_REVISION_LEAD_MS: i64 = 60 * 60 * 1_000;
pub const MAX_VALID_TIME_MS: i64 = i64::MAX - MAX_REVISION_LEAD_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamLabel {
    Repository,
    Session,
}

impl StreamLabel {
    /// The wire name; the linearization key orders streams by it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Session => "session",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub String);

impl EventId {
    /// Ids are derived from the event's own key, never from its position.
    pub fn derive(stream: StreamLabel, entity_id: &str, local_seq: u32) -> Self {
        Self(format!("{}:{entity_id}:{local_seq}", stream.label()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: EventId,
    pub stream: StreamLabel,
    pub entity_id: String,
    pub local_seq: u32,
    #[serde(with = "crate::decimal")]
    pub valid_time_ms: i64,
    #[serde(with = "crate::decimal")]
    pub observation_time_ms: i64,
    pub causal_depth: u32,
    pub payload: Payload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Payload {
    Message {
        message_id: String,
        role: String,
        text: String,
        cites: Option<EventId>,
    },
    ToolSpan {
        message_id: String,
        call_id: String,
        output: String,
    },
    Commit {
        oid: String,
        message: String,
    },
    Rename {
        oid: String,
        from_path: String,
        to_path: String,
        previous: Option<EventId>,
    },
    Correction {
        target: EventId,
        text: String,
    },
    Invalidation {
        target: EventId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalEdge {
    pub from: EventId,
    pub to: EventId,
}

/// `events` are in linearization order; `causal_edges` are sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventLog {
    pub schema: String,
    pub linearization_rule_version: String,
    pub events: Vec<Event>,
    pub causal_edges: Vec<CausalEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    SchemaMismatch {
        field: &'static str,
        found: String,
    },
    EventBound {
        events: usize,
        max: usize,
    },
    IdNotDerived {
        id: EventId,
    },
    DuplicateId {
        id: EventId,
    },
    NotLinearized {
        position: usize,
    },
    TimeOutOfDomain {
        id: EventId,
    },
    RevisionAhead {
        id: EventId,
    },
    DanglingEdge {
        edge: CausalEdge,
    },
    EdgeAgainstOrder {
        edge: CausalEdge,
    },
    EdgeAgainstDepth {
        edge: CausalEdge,
    },
    EdgesNotSorted {
        position: usize,
    },
    /// A payload names an event the log does not hold.
    DanglingReference {
        id: EventId,
        target: EventId,
    },
    /// `:` separates the parts of a derived ID, so a tag holding one could
    /// mint one ID for two entities.
    InvalidEntityTag {
        tag: String,
    },
}

debug_display!(LogError);

impl Payload {
    /// The event this payload points at: a cited message, a rename's
    /// predecessor, or a correction's or retraction's target. Every variant
    /// is listed so a new reference-bearing one cannot hide behind a wildcard.
    pub fn reference(&self) -> Option<&EventId> {
        match self {
            Payload::Message { cites, .. } => cites.as_ref(),
            Payload::Rename { previous, .. } => previous.as_ref(),
            Payload::Correction { target, .. } | Payload::Invalidation { target } => Some(target),
            Payload::ToolSpan { .. } | Payload::Commit { .. } => None,
        }
    }

    pub fn reference_mut(&mut self) -> Option<&mut EventId> {
        match self {
            Payload::Message { cites, .. } => cites.as_mut(),
            Payload::Rename { previous, .. } => previous.as_mut(),
            Payload::Correction { target, .. } | Payload::Invalidation { target } => Some(target),
            Payload::ToolSpan { .. } | Payload::Commit { .. } => None,
        }
    }

    /// The event this payload makes non-current, if it is a correction or a
    /// retraction.
    pub fn supersedes(&self) -> Option<&EventId> {
        match self {
            Payload::Correction { target, .. } | Payload::Invalidation { target } => Some(target),
            Payload::Message { .. }
            | Payload::ToolSpan { .. }
            | Payload::Commit { .. }
            | Payload::Rename { .. } => None,
        }
    }

    /// The payload with every event reference erased, for comparing two
    /// histories by what they say rather than by whom they name.
    pub fn content(&self) -> Payload {
        let mut content = self.clone();
        if let Some(target) = content.reference_mut() {
            *target = EventId(String::new());
        }
        content
    }
}

impl Event {
    pub fn key(&self) -> (i64, u32, &'static str, &str, u32) {
        (
            self.valid_time_ms,
            self.causal_depth,
            self.stream.label(),
            &self.entity_id,
            self.local_seq,
        )
    }
}

impl EventLog {
    pub(crate) fn new(mut events: Vec<Event>, mut causal_edges: Vec<CausalEdge>) -> Self {
        events.sort_by(|a, b| a.key().cmp(&b.key()));
        causal_edges.sort();
        Self {
            schema: EVENT_SCHEMA_VERSION.to_string(),
            linearization_rule_version: LINEARIZATION_RULE_VERSION.to_string(),
            events,
            causal_edges,
        }
    }

    /// Removes one event and its incident edges; payloads naming it are untouched.
    pub fn without(&self, id: &EventId) -> Self {
        let mut log = self.clone();
        log.events.retain(|event| event.id != *id);
        log.causal_edges
            .retain(|edge| edge.from != *id && edge.to != *id);
        log
    }

    /// Moves every event onto entities suffixed `~tag`, re-deriving each ID
    /// and following every payload reference and causal edge, so a history
    /// authored apart from another can share a log with it without an
    /// identity collision. A reference to an event the log does not hold is
    /// refused rather than left pointing into whatever log this one joins.
    pub fn on_distinct_entities(&self, tag: &str) -> Result<Self, LogError> {
        if tag.contains(':') {
            return Err(LogError::InvalidEntityTag {
                tag: tag.to_string(),
            });
        }
        let renamed: BTreeMap<&EventId, EventId> = self
            .events
            .iter()
            .map(|event| {
                let entity = format!("{}~{tag}", event.entity_id);
                (
                    &event.id,
                    EventId::derive(event.stream, &entity, event.local_seq),
                )
            })
            .collect();
        let follow = |id: &EventId| renamed.get(id).cloned();
        let mut events = Vec::with_capacity(self.events.len());
        for event in &self.events {
            let mut moved = event.clone();
            moved.entity_id = format!("{}~{tag}", event.entity_id);
            moved.id = follow(&event.id).expect("every event renames itself");
            if let Some(target) = moved.payload.reference_mut() {
                *target = follow(target).ok_or_else(|| LogError::DanglingReference {
                    id: event.id.clone(),
                    target: target.clone(),
                })?;
            }
            events.push(moved);
        }
        let edges = self
            .causal_edges
            .iter()
            .map(|edge| {
                Some(CausalEdge {
                    from: follow(&edge.from)?,
                    to: follow(&edge.to)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .expect("validated edges name held events");
        Ok(Self::new(events, edges))
    }

    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self).expect("log serializes");
        protocol_digest(LOG_DIGEST_PROTOCOL, &value).expect("log is canonical")
    }

    pub fn validate(&self, max_events: usize) -> Result<(), LogError> {
        if self.schema != EVENT_SCHEMA_VERSION {
            return Err(LogError::SchemaMismatch {
                field: "schema",
                found: self.schema.clone(),
            });
        }
        if self.linearization_rule_version != LINEARIZATION_RULE_VERSION {
            let found = self.linearization_rule_version.clone();
            return Err(LogError::SchemaMismatch {
                field: "linearization_rule_version",
                found,
            });
        }
        if self.events.len() > max_events {
            return Err(LogError::EventBound {
                events: self.events.len(),
                max: max_events,
            });
        }
        let mut positions: BTreeMap<&EventId, (usize, u32)> = BTreeMap::new();
        for (position, event) in self.events.iter().enumerate() {
            if event.id != EventId::derive(event.stream, &event.entity_id, event.local_seq) {
                return Err(LogError::IdNotDerived {
                    id: event.id.clone(),
                });
            }
            if position > 0 && self.events[position - 1].key() >= event.key() {
                return Err(LogError::NotLinearized { position });
            }
            if !(0..=MAX_VALID_TIME_MS).contains(&event.valid_time_ms)
                || event.observation_time_ms < 0
            {
                return Err(LogError::TimeOutOfDomain {
                    id: event.id.clone(),
                });
            }
            if event.valid_time_ms
                > event
                    .observation_time_ms
                    .saturating_add(MAX_REVISION_LEAD_MS)
            {
                return Err(LogError::RevisionAhead {
                    id: event.id.clone(),
                });
            }
            if positions
                .insert(&event.id, (position, event.causal_depth))
                .is_some()
            {
                return Err(LogError::DuplicateId {
                    id: event.id.clone(),
                });
            }
        }
        for (position, edge) in self.causal_edges.iter().enumerate() {
            if position > 0 && self.causal_edges[position - 1] >= *edge {
                return Err(LogError::EdgesNotSorted { position });
            }
            let (Some(from), Some(to)) = (positions.get(&edge.from), positions.get(&edge.to))
            else {
                return Err(LogError::DanglingEdge { edge: edge.clone() });
            };
            if from.0 >= to.0 {
                return Err(LogError::EdgeAgainstOrder { edge: edge.clone() });
            }
            if from.1 >= to.1 {
                return Err(LogError::EdgeAgainstDepth { edge: edge.clone() });
            }
        }
        Ok(())
    }
}
