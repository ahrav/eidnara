//! Replays source-corpus sessions through the daemon's harness source adapters and the screened
//! publisher against a real kernel, and reconciles every expected unit the independent readers
//! name (`crates/daemon/src/source_corpus.rs`) against what the replay admitted.
//!
//! A replay pass offers every unit not yet settled to [`publish_screened`] as one batch; a unit
//! the batch defers is offered again by the next pass, and a unit still deferred after the last
//! pass is the `deferred` loss. Shared by `tests/source_corpus_replay.rs` and the
//! `source_corpus_report` example.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use daemon::harness_sources::{
    AdapterRefusal, Harness, PublishError, SessionIdentity, SourcePublisher, SourceUnit,
    UnitDisposition, opencode_units, pi_units, publish_screened,
};
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactErrorKind, CommitIntent, Dimension, DomainSpec, KernelStore, ProviderEgress, ScopeSpec,
    ScopeTermSpec, Sensitivity,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::source_corpus::{ExpectedRecord, ExpectedUnit, LossCondition};

/// The project digest every replayed session is bound to.
pub const PROJECT: &str = "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0";
const DOMAIN: &str = "corpus-domain";
const SCOPE: &str = "project:corpus";

/// A kernel with one scoped project, the store every replay in a process publishes into.
pub struct ReplayStore {
    pub kernel: Arc<KernelStore>,
}

impl ReplayStore {
    pub fn open(root: &Path) -> Self {
        let kernel = Arc::new(KernelStore::open(root.join("kernel")).expect("kernel opens"));
        let intent = CommitIntent {
            producer: "source-corpus-replay".to_owned(),
            operation_key: "seed".to_owned(),
            request_digest: format!("{:x}", Sha256::digest(b"seed")),
            actor: "corpus".to_owned(),
            cause: "replay".to_owned(),
        };
        kernel
            .commit(intent, |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_owned(),
                    object_id: "corpus-domain-object".to_owned(),
                    name: "Corpus".to_owned(),
                    source_kind: "fixture".to_owned(),
                    source_id: DOMAIN.to_owned(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_owned(),
                    object_id: SCOPE.to_owned(),
                    source_id: SCOPE.to_owned(),
                    domain_id: DOMAIN.to_owned(),
                    source_kind: "kernel_route".to_owned(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![ScopeTermSpec {
                        dimension: Dimension::Project.as_str().to_owned(),
                        operator: "exact".to_owned(),
                        exact_value: Some(PROJECT.to_owned()),
                        ..ScopeTermSpec::default()
                    }],
                })?;
                Ok(String::new())
            })
            .expect("seed commits");
        Self { kernel }
    }

    fn publisher(&self) -> SourcePublisher<'_> {
        SourcePublisher {
            kernel: &self.kernel,
            domain_id: DOMAIN,
            scope_id: Some(SCOPE),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Normal,
        }
    }
}

/// One native record in the shape the harness plugin sends, with the expected record its reader
/// produced for it.
pub struct ReplayRecord {
    pub native: Value,
    pub expected: ExpectedRecord,
}

/// The OpenCode `MessageV2` records of a frozen session: each `message` row's `data` with its
/// `id` and `sessionID`, and each part's `data` with its ids, as the plugin presents them.
pub fn opencode_natives(fixture: &Value) -> Vec<Value> {
    let session = fixture["session_id"].as_str().expect("fixture session id");
    fixture["messages"]
        .as_array()
        .expect("fixture messages")
        .iter()
        .map(|message| {
            let id = message["id"].as_str().expect("message id");
            let mut info: Map<String, Value> =
                message["data"].as_object().expect("message data").clone();
            info.insert("id".to_owned(), Value::from(id));
            info.insert("sessionID".to_owned(), Value::from(session));
            let mut parts: Vec<(i64, String, Value)> = message["parts"]
                .as_array()
                .expect("message parts")
                .iter()
                .map(|part| {
                    let mut data = part["data"].as_object().expect("part data").clone();
                    let part_id = part["id"].as_str().expect("part id").to_owned();
                    data.insert("id".to_owned(), Value::from(part_id.as_str()));
                    data.insert("messageID".to_owned(), Value::from(id));
                    data.insert("sessionID".to_owned(), Value::from(session));
                    let created = part["time_created"].as_i64().unwrap_or_default();
                    (created, part_id, Value::Object(data))
                })
                .collect();
            parts.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            serde_json::json!({
                "info": info,
                "parts": parts.into_iter().map(|(_, _, part)| part).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// The native id of the record an adapter reads: the OpenCode `info.id` or the Pi entry `id`.
fn native_id(native: &Value) -> &str {
    native
        .pointer("/info/id")
        .or_else(|| native.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Pairs native records with their expected records by native id; a native record the reader
/// left out (an OpenCode compaction summary) is replayed with no expected unit.
pub fn pair(natives: Vec<Value>, expected: Vec<ExpectedRecord>) -> Vec<ReplayRecord> {
    let mut by_id: BTreeMap<String, ExpectedRecord> = expected
        .into_iter()
        .map(|record| (record.id.clone(), record))
        .collect();
    natives
        .into_iter()
        .map(|native| {
            let id = native_id(&native).to_owned();
            let expected = by_id.remove(&id).unwrap_or(ExpectedRecord {
                id,
                role: None,
                units: Vec::new(),
                non_units: Vec::new(),
            });
            ReplayRecord { native, expected }
        })
        .collect()
}

/// The adapter a replay feeds each native record through; a test substitutes a mutated one.
pub type Adapter<'a> =
    &'a dyn Fn(&SessionIdentity, &Value) -> Result<Vec<SourceUnit>, AdapterRefusal>;

pub fn harness_adapter(
    session: &SessionIdentity,
    native: &Value,
) -> Result<Vec<SourceUnit>, AdapterRefusal> {
    match session.harness {
        Harness::OpenCode => opencode_units(session, native),
        Harness::Pi => pi_units(session, native),
    }
}

/// The reconciliation key of a unit: its class, its native record or call id, and its block.
fn unit_key(unit: &SourceUnit) -> String {
    let field = |name: &str| {
        unit.identity
            .iter()
            .find(|(field, _)| *field == name)
            .map_or("", |(_, value)| value.as_str())
    };
    match unit.class {
        OccurrenceClass::RawToolSpans => {
            format!("tool:{}:{}", field("tool_call_id"), field("block_index"))
        }
        _ => format!("text:{}:{}", field("message_id"), field("block_index")),
    }
}

fn expected_key(record: &ExpectedRecord, unit: &ExpectedUnit) -> String {
    match &unit.tool {
        Some(tool) => format!("tool:{}:{}", tool.call_id, unit.block),
        None => format!("text:{}:{}", record.id, unit.block),
    }
}

/// How one replayed unit ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    Admitted {
        replayed: bool,
    },
    Lost(LossCondition),
    /// A refusal no loss condition names.
    Unnamed,
}

fn settled(disposition: &UnitDisposition) -> Option<Settled> {
    Some(match disposition {
        UnitDisposition::Published(published) => Settled::Admitted {
            replayed: published.replayed,
        },
        UnitDisposition::UnitTooLarge => Settled::Lost(LossCondition::UnitTooLarge),
        UnitDisposition::Deferred => return None,
        UnitDisposition::Superseded => Settled::Lost(LossCondition::Superseded),
        UnitDisposition::Refused(error) => match error {
            PublishError::Artifact(
                ArtifactErrorKind::ExactBytesRewritten
                | ArtifactErrorKind::UnredactableSecret
                | ArtifactErrorKind::ScanIncomplete
                | ArtifactErrorKind::DetectionLimit,
            ) => Settled::Lost(LossCondition::SecretBearing),
            PublishError::Artifact(ArtifactErrorKind::ReAdmissionBlocked) => {
                Settled::Lost(LossCondition::ReAdmissionBlocked)
            }
            PublishError::IdentityReused | PublishError::Descriptor { .. } => {
                Settled::Lost(LossCondition::IdentityConflict)
            }
            error if !error.is_permanent() => Settled::Lost(LossCondition::RefusedRetryable),
            _ => Settled::Unnamed,
        },
    })
}

/// One replayed unit: its key, the expected unit it answers, and how it ended.
#[derive(Debug, Clone)]
pub struct ReplayedUnit {
    pub key: String,
    pub revision: String,
    pub bytes: usize,
    pub settled: Settled,
}

/// One pass's batch: the units it offered and admitted, their bytes, and the publish time.
#[derive(Debug, Clone, Copy)]
pub struct PassMeasure {
    pub offered_units: usize,
    pub offered_bytes: usize,
    pub admitted_units: usize,
    pub admitted_bytes: usize,
    pub elapsed_ms: f64,
}

pub struct Replay {
    pub units: Vec<ReplayedUnit>,
    /// Records the adapter refused whole, by native id.
    pub refused_records: BTreeSet<String>,
    pub passes: Vec<PassMeasure>,
}

/// Replays `records` of one session: each through `adapter`, then up to `max_passes` screened
/// batches of every unit not yet settled.
pub fn replay(
    store: &ReplayStore,
    session: &SessionIdentity,
    records: &[ReplayRecord],
    adapter: Adapter<'_>,
    max_passes: usize,
) -> Replay {
    let mut units: Vec<(SourceUnit, Option<Settled>)> = Vec::new();
    let mut refused_records = BTreeSet::new();
    for record in records {
        match adapter(session, &record.native) {
            Ok(produced) => units.extend(produced.into_iter().map(|unit| (unit, None))),
            Err(_) => {
                refused_records.insert(native_id(&record.native).to_owned());
            }
        }
    }
    let publisher = store.publisher();
    let mut passes = Vec::new();
    for _ in 0..max_passes {
        let open: Vec<usize> = (0..units.len())
            .filter(|&index| units[index].1.is_none())
            .collect();
        if open.is_empty() {
            break;
        }
        let batch: Vec<SourceUnit> = open.iter().map(|&index| units[index].0.clone()).collect();
        let observed_at = batch
            .iter()
            .filter_map(|unit| unit.revision.parse::<i64>().ok())
            .max()
            .unwrap_or(0);
        let start = Instant::now();
        let outcome = publish_screened(&publisher, &batch, observed_at, None);
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        let mut measure = PassMeasure {
            offered_units: batch.len(),
            offered_bytes: batch.iter().map(|unit| unit.text.len()).sum(),
            admitted_units: 0,
            admitted_bytes: 0,
            elapsed_ms,
        };
        match outcome {
            Ok(dispositions) => {
                for (&index, disposition) in open.iter().zip(&dispositions) {
                    let outcome = settled(disposition);
                    if let Some(Settled::Admitted { .. }) = outcome {
                        measure.admitted_units += 1;
                        measure.admitted_bytes += units[index].0.text.len();
                    }
                    units[index].1 = outcome;
                }
            }
            Err(_) => {
                for &index in &open {
                    units[index].1 = Some(Settled::Lost(LossCondition::PassFailed));
                }
            }
        }
        passes.push(measure);
    }
    Replay {
        units: units
            .into_iter()
            .map(|(unit, outcome)| ReplayedUnit {
                key: unit_key(&unit),
                revision: unit.revision.clone(),
                bytes: unit.text.len(),
                settled: outcome.unwrap_or(Settled::Lost(LossCondition::Deferred)),
            })
            .collect(),
        refused_records,
        passes,
    }
}

/// Every expected unit accounted once: admitted, or lost under one named condition.
#[derive(Debug, Default)]
pub struct Reconciliation {
    pub expected: usize,
    pub admitted: usize,
    pub replayed: usize,
    pub losses: BTreeMap<LossCondition, usize>,
    /// Expected units with no admission and no named loss, each naming its record and block.
    pub unclassified: Vec<String>,
    /// Replayed units no expected unit names.
    pub phantom: Vec<String>,
    /// Expected units whose replayed revision differs from the one the harness wrote.
    pub revision_mismatch: Vec<String>,
}

impl Reconciliation {
    pub fn lost(&self) -> usize {
        self.losses.values().sum()
    }
}

pub fn reconcile(records: &[ReplayRecord], replay: &Replay) -> Reconciliation {
    let by_key: BTreeMap<&str, &ReplayedUnit> = replay
        .units
        .iter()
        .map(|unit| (unit.key.as_str(), unit))
        .collect();
    let mut result = Reconciliation::default();
    let mut expected_keys = BTreeSet::new();
    for record in records {
        let expected = &record.expected;
        for unit in &expected.units {
            result.expected += 1;
            let key = expected_key(expected, unit);
            expected_keys.insert(key.clone());
            if replay.refused_records.contains(&expected.id) {
                *result
                    .losses
                    .entry(LossCondition::AdapterRefused)
                    .or_default() += 1;
                continue;
            }
            let Some(replayed) = by_key.get(key.as_str()) else {
                result.unclassified.push(format!(
                    "{key} (record {}, part {})",
                    expected.id, unit.index
                ));
                continue;
            };
            if unit
                .revision
                .as_ref()
                .is_some_and(|revision| *revision != replayed.revision)
            {
                result.revision_mismatch.push(format!(
                    "{key} (record {}, part {}): expected revision {}, replayed {}",
                    expected.id,
                    unit.index,
                    unit.revision.as_deref().unwrap_or_default(),
                    replayed.revision
                ));
            }
            match replayed.settled {
                Settled::Admitted { replayed } => {
                    result.admitted += 1;
                    result.replayed += usize::from(replayed);
                }
                Settled::Lost(condition) => *result.losses.entry(condition).or_default() += 1,
                Settled::Unnamed => result.unclassified.push(format!(
                    "{key} (record {}, part {}): unnamed refusal",
                    expected.id, unit.index
                )),
            }
        }
    }
    result.phantom = replay
        .units
        .iter()
        .filter(|unit| !expected_keys.contains(&unit.key))
        .map(|unit| unit.key.clone())
        .collect();
    result
}
