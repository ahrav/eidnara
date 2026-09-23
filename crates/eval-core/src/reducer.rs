use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::eligibility::{
    Destination, FactTuple, Sensitivity, ServedClass, SpecError, StateFacts, Verdict, check_spec,
    judge_with,
};
use crate::event::{EventId, EventLog, LogError, MAX_VALID_TIME_MS, Payload, Supersession};

pub const REDUCER_VERSION: &str = "eval-reducer/v1";

/// A bitemporal cut plus the facts the world does not carry: an event is true
/// at `valid_time_ms` and known at `observation_time_ms`. `served` and
/// `registry_sensitivity` apply to every unit alike, so `hidden` and
/// `provider_sensitive` are world-wide switches here, not per-unit facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    #[serde(with = "crate::decimal")]
    pub valid_time_ms: i64,
    #[serde(with = "crate::decimal")]
    pub observation_time_ms: i64,
    /// Entities the queried project scope names.
    pub scope: BTreeSet<String>,
    pub destination: Destination,
    /// The serving class ingestion gives every unit; `None` is unadmitted.
    pub served: Option<ServedClass>,
    pub registry_sensitivity: Sensitivity,
    pub max_events_per_log: u32,
}

impl Query {
    pub fn validate(&self) -> Result<(), ReduceError> {
        let invalid = [
            (
                "valid_time_ms",
                !(0..=MAX_VALID_TIME_MS).contains(&self.valid_time_ms),
            ),
            ("observation_time_ms", self.observation_time_ms < 0),
            ("max_events_per_log", self.max_events_per_log == 0),
        ];
        match invalid.into_iter().find(|(_, invalid)| *invalid) {
            Some((field, _)) => Err(ReduceError::InvalidQuery(field)),
            None => Ok(()),
        }
    }
}

/// One unit judged at its own revision, so `stale` is out of reach here; the
/// shell produces it by asking about an older revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitTruth {
    pub facts: FactTuple,
    pub verdict: Verdict,
}

/// One unit per non-invalidation event; `required` holds the units the
/// pinned rules judge `Ok`, which is exactly when a historical question about
/// them must stay answerable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Truth {
    /// The shell copies this into the manifest's `component_versions.reducer`.
    pub reducer_version: String,
    pub query: Query,
    pub units: BTreeMap<EventId, UnitTruth>,
    pub required: BTreeSet<EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReduceError {
    InvalidQuery(&'static str),
    Spec(SpecError),
    Log(LogError),
}

debug_display!(ReduceError);

/// Refuses an invalid query, a drifted spec fixture, or an invalid log before
/// producing truth. A correction or invalidation whose target the log does
/// not contain changes nothing: deletion leaves such targets behind by design.
pub fn reduce(log: &EventLog, fixture: &Value, query: &Query) -> Result<Truth, ReduceError> {
    query.validate()?;
    let spec = check_spec(fixture).map_err(ReduceError::Spec)?;
    log.validate(query.max_events_per_log as usize)
        .map_err(ReduceError::Log)?;
    let in_cut = |event: &crate::event::Event| {
        event.valid_time_ms <= query.valid_time_ms
            && event.observation_time_ms <= query.observation_time_ms
    };
    let mut corrected = BTreeSet::new();
    let mut retracted = BTreeSet::new();
    for event in log.events.iter().filter(|event| in_cut(event)) {
        match event.payload.supersedes() {
            Some((Supersession::Correction, target)) => {
                corrected.insert(target.clone());
            }
            Some((Supersession::Retraction, target)) => {
                retracted.insert(target.clone());
            }
            None => {}
        }
    }
    let mut units = BTreeMap::new();
    let mut required = BTreeSet::new();
    for event in &log.events {
        if matches!(event.payload, Payload::Invalidation { .. }) {
            continue;
        }
        let superseded = corrected.contains(&event.id);
        let state = in_cut(event).then_some(StateFacts {
            superseded,
            invalidated: superseded || retracted.contains(&event.id),
            revision_matches: true,
            in_scope: query.scope.contains(&event.entity_id),
            registry_sensitivity: query.registry_sensitivity,
        });
        let facts = FactTuple {
            state,
            served: query.served,
            artifact: None,
            destination: query.destination,
        };
        let verdict = judge_with(&spec.predicates, &facts);
        if verdict == Verdict::Ok {
            required.insert(event.id.clone());
        }
        units.insert(event.id.clone(), UnitTruth { facts, verdict });
    }
    Ok(Truth {
        reducer_version: REDUCER_VERSION.to_string(),
        query: query.clone(),
        units,
        required,
    })
}
