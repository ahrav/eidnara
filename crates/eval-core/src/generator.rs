use std::collections::BTreeMap;

use context_core::canonical_json::protocol_digest;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::event::{
    CausalEdge, Event, EventId, EventLog, LogError, MAX_VALID_TIME_MS, Payload, StreamLabel,
};
use crate::injection::Carrier;
use crate::stream::{ChoiceKind, Chooser, RANDOM_SCHEMA_VERSION, ReplayRefusal, Tape};

/// Version 2 removed the zero time gap; version 3 gave every text a word of
/// its own and the world's own word beside the drawn word, so a surface that
/// matches on words can tell one message from another and from another
/// world's. The same seed and config produce a different world under each
/// version.
pub const GENERATOR_VERSION: &str = "eval-generator/v3";
pub const TAPE_IDENTITY_PROTOCOL: &str = "eval-tape/v1";
/// Separates what a generated text says from the world's own word after it.
const WORLD_WORD_SEPARATOR: &str = " in ";

/// The decision a generated text records: its words before the world's own
/// word, which is provenance rather than content.
pub fn text_decision(text: &str) -> &str {
    text.rsplit_once(WORLD_WORD_SEPARATOR)
        .map_or(text, |(decision, _)| decision)
}
const OID_PROTOCOL: &str = "eval-git-oid/v1";

/// Strictly positive, so a correction always advances its target's revision.
const TIME_GAP_TICKS: [i64; 3] = [1, 2, 5];
const _: () = assert!(all_positive(&TIME_GAP_TICKS));

const fn all_positive(ticks: &[i64]) -> bool {
    let mut i = 0;
    while i < ticks.len() {
        if ticks[i] <= 0 {
            return false;
        }
        i += 1;
    }
    true
}
const OBSERVATION_LAGS_MS: [i64; 4] = [0, 1_000, 60_000, 3_600_000];
const WORDS: [&str; 6] = [
    "allocator",
    "barrier",
    "cursor",
    "digest",
    "envelope",
    "fence",
];
const INITIAL_PATHS: [&str; 3] = ["src/lib.rs", "src/main.rs", "README.md"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldConfig {
    pub sessions: Vec<SessionSpec>,
    pub repositories: Vec<RepositorySpec>,
    #[serde(with = "crate::decimal")]
    pub epoch_ms: i64,
    #[serde(with = "crate::decimal")]
    pub tick_ms: i64,
    pub max_events_per_log: u32,
    /// Injection canaries planted into the text a carrier already emits; an
    /// empty list plants nothing and leaves the world as it was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planted: Vec<Planted>,
}

/// One injection canary planted into the text of one payload: a message's
/// text for the `summary` carrier (the summarizer folds it into a segment),
/// a tool span's output for `tool_output`, a commit's message for
/// `commit_message`. `entity` names the actor (`session-0`, `repository-0`)
/// and `slot` its `k`-th mutation. The generated world has no issue and no
/// memory payload, so those carriers cannot be planted and are refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Planted {
    pub carrier: Carrier,
    pub entity: String,
    pub slot: u32,
    pub canary: String,
}

/// `*_every` of `n` fires on every `n`-th slot; `0` never fires. Corrections and
/// invalidations also skip slot `0`, which has no earlier message to target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSpec {
    pub messages: u32,
    pub tool_span_every: u32,
    pub correction_every: u32,
    pub invalidation_every: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySpec {
    pub commits: u32,
    pub rename_every: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldError {
    InvalidField(&'static str),
    /// Raised before generation from the declared count, and while driving
    /// from the running count.
    EventBound {
        events: u64,
        max: u32,
    },
    TimeOverflow {
        entity_id: String,
    },
    Replay(ReplayRefusal),
    Log(LogError),
}

debug_display!(WorldError);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Generate,
    ReplayTape(Tape),
}

/// `Emitted` batches are never empty and arrive in emission order; `Done` repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Emitted(Vec<Event>),
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct World {
    pub log: EventLog,
    pub tape: Tape,
}

fn fires(every: u32, k: u32) -> bool {
    every > 0 && (k + 1).is_multiple_of(every)
}

/// Slots `k` in `0..slots` where `fires(every, k)`; `skip_first` drops slot `0`, which only `every == 1` reaches.
fn firings(every: u32, slots: u32, skip_first: bool) -> u64 {
    if every == 0 {
        return 0;
    }
    u64::from(slots / every) - u64::from(skip_first && every == 1 && slots > 0)
}

impl SessionSpec {
    fn events_at(&self, k: u32) -> usize {
        1 + usize::from(fires(self.tool_span_every, k))
            + usize::from(k > 0 && fires(self.correction_every, k))
            + usize::from(k > 0 && fires(self.invalidation_every, k))
    }

    fn declared_events(&self) -> u64 {
        u64::from(self.messages)
            + firings(self.tool_span_every, self.messages, false)
            + firings(self.correction_every, self.messages, true)
            + firings(self.invalidation_every, self.messages, true)
    }
}

impl RepositorySpec {
    fn events_at(&self, k: u32) -> usize {
        1 + usize::from(fires(self.rename_every, k))
    }

    fn declared_events(&self) -> u64 {
        u64::from(self.commits) + firings(self.rename_every, self.commits, false)
    }
}

/// Binds a tape to the seed, config, and generator it was recorded under.
pub fn tape_identity(root_seed: u64, config: &WorldConfig) -> String {
    let key = json!({
        "root_seed": root_seed.to_string(),
        "config": config,
        "generator_version": GENERATOR_VERSION,
        "random_schema_version": RANDOM_SCHEMA_VERSION,
    });
    protocol_digest(TAPE_IDENTITY_PROTOCOL, &key).expect("tape identity is canonical")
}

impl WorldConfig {
    /// The exact number of events generation emits, computed in `O(entities)`.
    pub fn declared_events(&self) -> u64 {
        let sessions = self.sessions.iter().map(SessionSpec::declared_events);
        let repositories = self
            .repositories
            .iter()
            .map(RepositorySpec::declared_events);
        sessions.chain(repositories).fold(0, u64::saturating_add)
    }

    pub fn validate(&self) -> Result<(), WorldError> {
        let empty_spec = self.sessions.iter().any(|spec| spec.messages == 0)
            || self.repositories.iter().any(|spec| spec.commits == 0);
        let no_entities = self.sessions.is_empty() && self.repositories.is_empty();
        let invalid = [
            ("entities", empty_spec || no_entities),
            ("tick_ms", self.tick_ms <= 0),
            (
                "epoch_ms",
                !(0..=MAX_VALID_TIME_MS).contains(&self.epoch_ms),
            ),
            ("max_events_per_log", self.max_events_per_log == 0),
        ];
        if let Some((field, _)) = invalid.into_iter().find(|(_, invalid)| *invalid) {
            return Err(WorldError::InvalidField(field));
        }
        for planted in &self.planted {
            let session = self
                .sessions
                .iter()
                .enumerate()
                .find(|(index, _)| format!("session-{index}") == planted.entity);
            let repository = self
                .repositories
                .iter()
                .enumerate()
                .find(|(index, _)| format!("repository-{index}") == planted.entity);
            let plantable = match (planted.carrier, session, repository) {
                (Carrier::Summary, Some((_, spec)), None) => planted.slot < spec.messages,
                (Carrier::ToolOutput, Some((_, spec)), None) => {
                    planted.slot < spec.messages && fires(spec.tool_span_every, planted.slot)
                }
                (Carrier::CommitMessage, None, Some((_, spec))) => planted.slot < spec.commits,
                _ => false,
            };
            if planted.canary.is_empty() || !plantable {
                return Err(WorldError::InvalidField("planted"));
            }
        }
        let max = self.max_events_per_log;
        let events = self.declared_events();
        if events > u64::from(max) {
            return Err(WorldError::EventBound { events, max });
        }
        Ok(())
    }
}

/// One entity's `k`-th mutation; `entity` indexes `Generator::entities`, `spec` the stream's list.
#[derive(Clone)]
struct Slot {
    stream: StreamLabel,
    entity: usize,
    spec: usize,
    actor: String,
    k: u32,
    valid_time_ms: i64,
}

#[derive(Default)]
struct EntityState {
    seq: u32,
    messages: Vec<EventId>,
    last_commit: Option<EventId>,
    paths: Vec<(String, Option<EventId>)>,
}

pub struct Generator {
    config: WorldConfig,
    /// The word every text in this world carries and no other world does.
    vocabulary: String,
    chooser: Chooser,
    schedule: Vec<Slot>,
    cursor: usize,
    events: Vec<Event>,
    edges: Vec<CausalEdge>,
    depths: BTreeMap<EventId, u32>,
    commits: Vec<EventId>,
    entities: Vec<EntityState>,
    failed: Option<WorldError>,
}

impl Generator {
    /// Draws every slot time up front, so the cost is `O(declared slots)`.
    pub fn new(root_seed: u64, config: &WorldConfig, mode: Mode) -> Result<Self, WorldError> {
        config.validate()?;
        let identity = tape_identity(root_seed, config);
        let chooser = match mode {
            Mode::Generate => Chooser::generate(root_seed, identity),
            Mode::ReplayTape(tape) => {
                Chooser::replay(tape, identity).map_err(WorldError::Replay)?
            }
        };
        let entities = config.sessions.len() + config.repositories.len();
        let mut generator = Self {
            vocabulary: format!("world{root_seed:016x}"),
            schedule: Vec::new(),
            cursor: 0,
            events: Vec::new(),
            edges: Vec::new(),
            depths: BTreeMap::new(),
            commits: Vec::new(),
            entities: (0..entities).map(|_| EntityState::default()).collect(),
            failed: None,
            chooser,
            config: config.clone(),
        };
        for entity in 0..entities {
            generator.schedule_entity(entity)?;
        }
        generator
            .schedule
            .sort_by_key(|slot| (slot.valid_time_ms, slot.stream, slot.entity, slot.k));
        Ok(generator)
    }

    /// Slot times are chosen per entity, so no entity's schedule depends on another's slots.
    fn schedule_entity(&mut self, entity: usize) -> Result<(), WorldError> {
        let sessions = self.config.sessions.len();
        let (stream, spec, slots) = match entity < sessions {
            true => (
                StreamLabel::Session,
                entity,
                self.config.sessions[entity].messages,
            ),
            false => (
                StreamLabel::Repository,
                entity - sessions,
                self.config.repositories[entity - sessions].commits,
            ),
        };
        let actor = format!("{}-{spec}", stream.label());
        if stream == StreamLabel::Repository {
            self.entities[entity].paths = INITIAL_PATHS
                .iter()
                .map(|path| (path.to_string(), None))
                .collect();
        }
        let mut valid_time_ms = self.config.epoch_ms;
        for k in 0..slots {
            let mut slot = Slot {
                stream,
                entity,
                spec,
                actor: actor.clone(),
                k,
                valid_time_ms,
            };
            if k > 0 {
                let gap = self.choose(ChoiceKind::TimeGap, &slot, &TIME_GAP_TICKS)?;
                valid_time_ms = TIME_GAP_TICKS[gap]
                    .checked_mul(self.config.tick_ms)
                    .and_then(|gap| valid_time_ms.checked_add(gap))
                    .filter(|time| *time <= MAX_VALID_TIME_MS)
                    .ok_or_else(|| WorldError::TimeOverflow {
                        entity_id: actor.clone(),
                    })?;
                slot.valid_time_ms = valid_time_ms;
            }
            self.schedule.push(slot);
        }
        Ok(())
    }

    /// Emits one mutation's events and returns at quiescence; batches are provisional
    /// until `finish` succeeds, and after an error every later call returns that error.
    pub fn step(&mut self) -> Result<Step, WorldError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let Some(slot) = self.schedule.get(self.cursor).cloned() else {
            return Ok(Step::Done);
        };
        let first = self.events.len();
        let result = match slot.stream {
            StreamLabel::Session => self.message_slot(slot),
            StreamLabel::Repository => self.commit_slot(slot),
        };
        match result {
            Ok(()) => {
                self.cursor += 1;
                Ok(Step::Emitted(self.events[first..].to_vec()))
            }
            Err(error) => {
                self.failed = Some(error.clone());
                Err(error)
            }
        }
    }

    /// The events emitted so far, re-linearized; an earlier snapshot is not a prefix of a later one.
    pub fn log(&self) -> Result<EventLog, WorldError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        Ok(EventLog::new(self.events.clone(), self.edges.clone()))
    }

    /// Drives the remaining slots, then closes the tape and validates the log.
    pub fn finish(mut self) -> Result<World, WorldError> {
        while let Step::Emitted(_) = self.step()? {}
        let log = EventLog::new(self.events, self.edges);
        log.validate(self.config.max_events_per_log as usize)
            .map_err(WorldError::Log)?;
        Ok(World {
            log,
            tape: self.chooser.finish(),
        })
    }

    /// Reserves the slot's events first; every lag is at most `MAX_REVISION_LEAD_MS`, so no overflow.
    fn open(&mut self, slot: &Slot, count: usize) -> Result<i64, WorldError> {
        let max = self.config.max_events_per_log;
        let events = (self.events.len() + count) as u64;
        if events > u64::from(max) {
            return Err(WorldError::EventBound { events, max });
        }
        let lag = self.choose(ChoiceKind::ObservationLag, slot, &OBSERVATION_LAGS_MS)?;
        Ok(slot.valid_time_ms + OBSERVATION_LAGS_MS[lag])
    }

    fn choose<T: Serialize>(
        &mut self,
        kind: ChoiceKind,
        slot: &Slot,
        candidates: &[T],
    ) -> Result<usize, WorldError> {
        let site = format!("slot:{}", slot.k);
        self.chooser
            .choose(kind, &slot.actor, &site, candidates)
            .map_err(WorldError::Replay)
    }

    /// One drawn word, one word only this slot has within its entity, and the
    /// world's own word: a lexical matcher needs two tokens of three characters
    /// or more, one of them rare, to find a message by its own text, and a
    /// message carried into another world must not read as one of that
    /// world's. Entities number their slots from zero, so another entity's
    /// same-index text shares the slot word; a surface that presents one
    /// session and no commit never meets the collision, and naming the entity
    /// in the token lengthens every text past the geometry the S0 campaign
    /// pins.
    fn text(&mut self, slot: &Slot) -> Result<String, WorldError> {
        let word = self.choose(ChoiceKind::TextWord, slot, &WORDS)?;
        Ok(format!(
            "{} for slot{}{WORLD_WORD_SEPARATOR}{}",
            WORDS[word], slot.k, self.vocabulary
        ))
    }

    fn emit(
        &mut self,
        slot: &Slot,
        observation_time_ms: i64,
        payload: Payload,
        predecessors: &[EventId],
    ) -> EventId {
        let depth = predecessors
            .iter()
            .map(|id| self.depths[id] + 1)
            .max()
            .unwrap_or(0);
        let seq = self.entities[slot.entity].seq;
        self.entities[slot.entity].seq += 1;
        let id = EventId::derive(slot.stream, &slot.actor, seq);
        self.edges
            .extend(predecessors.iter().map(|from| CausalEdge {
                from: from.clone(),
                to: id.clone(),
            }));
        self.events.push(Event {
            id: id.clone(),
            stream: slot.stream,
            entity_id: slot.actor.clone(),
            local_seq: seq,
            valid_time_ms: slot.valid_time_ms,
            observation_time_ms,
            causal_depth: depth,
            payload,
        });
        self.depths.insert(id.clone(), depth);
        id
    }

    /// The canary planted on this slot for `carrier`, appended to the text
    /// the carrier emits.
    fn planted(&self, slot: &Slot, carrier: Carrier, text: String) -> String {
        self.config
            .planted
            .iter()
            .filter(|planted| {
                planted.carrier == carrier && planted.entity == slot.actor && planted.slot == slot.k
            })
            .fold(text, |text, planted| format!("{text} {}", planted.canary))
    }

    fn message_slot(&mut self, slot: Slot) -> Result<(), WorldError> {
        let spec = self.config.sessions[slot.spec].clone();
        let k = slot.k;
        let observation = self.open(&slot, spec.events_at(k))?;
        let text = self.text(&slot)?;
        let text = self.planted(&slot, Carrier::Summary, text);
        let earlier = self.entities[slot.entity].messages.clone();
        let commits = self.commits.clone();
        let cites = match commits.is_empty() {
            true => None,
            false => Some(commits[self.choose(ChoiceKind::Cites, &slot, &commits)?].clone()),
        };
        let message_id = format!("{}-m{k}", slot.actor);
        let predecessors: Vec<EventId> = cites.iter().cloned().collect();
        let role = if k.is_multiple_of(2) {
            "user"
        } else {
            "assistant"
        };
        let payload = Payload::Message {
            message_id: message_id.clone(),
            role: role.to_string(),
            text,
            cites,
        };
        let message = self.emit(&slot, observation, payload, &predecessors);
        if fires(spec.tool_span_every, k) {
            let output = self.text(&slot)?;
            let output = self.planted(&slot, Carrier::ToolOutput, output);
            let payload = Payload::ToolSpan {
                call_id: format!("{message_id}-call0"),
                message_id,
                output,
            };
            self.emit(&slot, observation, payload, std::slice::from_ref(&message));
        }
        let revisions = [
            (ChoiceKind::CorrectionTarget, spec.correction_every),
            (ChoiceKind::InvalidationTarget, spec.invalidation_every),
        ];
        for (kind, every) in revisions {
            if k == 0 || !fires(every, k) {
                continue;
            }
            let target = earlier[self.choose(kind, &slot, &earlier)?].clone();
            let payload = match kind {
                ChoiceKind::CorrectionTarget => Payload::Correction {
                    target: target.clone(),
                    text: self.text(&slot)?,
                },
                ChoiceKind::InvalidationTarget => Payload::Invalidation {
                    target: target.clone(),
                },
                other => unreachable!("{other:?} is not a revision"),
            };
            self.emit(&slot, observation, payload, &[target]);
        }
        self.entities[slot.entity].messages.push(message);
        Ok(())
    }

    fn commit_slot(&mut self, slot: Slot) -> Result<(), WorldError> {
        let spec = self.config.repositories[slot.spec].clone();
        let k = slot.k;
        let observation = self.open(&slot, spec.events_at(k))?;
        let message = self.text(&slot)?;
        let message = self.planted(&slot, Carrier::CommitMessage, message);
        let oid_key = json!({"repository": slot.actor, "seq": k});
        let oid = protocol_digest(OID_PROTOCOL, &oid_key).expect("oid key is canonical")[..40]
            .to_string();
        let parent: Vec<EventId> = self.entities[slot.entity]
            .last_commit
            .iter()
            .cloned()
            .collect();
        let commit = self.emit(
            &slot,
            observation,
            Payload::Commit {
                oid: oid.clone(),
                message,
            },
            &parent,
        );
        if fires(spec.rename_every, k) {
            let paths: Vec<String> = self.entities[slot.entity]
                .paths
                .iter()
                .map(|(path, _)| path.clone())
                .collect();
            let index = self.choose(ChoiceKind::RenameTarget, &slot, &paths)?;
            let (from_path, previous) = self.entities[slot.entity].paths[index].clone();
            let to_path = format!("renamed/{k}/{from_path}");
            let predecessors: Vec<EventId> = std::iter::once(commit.clone())
                .chain(previous.clone())
                .collect();
            let payload = Payload::Rename {
                oid,
                from_path,
                to_path: to_path.clone(),
                previous,
            };
            let rename = self.emit(&slot, observation, payload, &predecessors);
            self.entities[slot.entity].paths[index] = (to_path, Some(rename));
        }
        self.entities[slot.entity].last_commit = Some(commit.clone());
        self.commits.push(commit);
        Ok(())
    }
}

pub fn generate_all(root_seed: u64, config: &WorldConfig, mode: Mode) -> Result<World, WorldError> {
    Generator::new(root_seed, config, mode)?.finish()
}
