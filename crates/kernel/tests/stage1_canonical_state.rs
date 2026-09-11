//! Canonical state survives a graceful close and reopen, and the outbox
//! replays into the same registry. The reopen reads the same WAL, so these
//! tests prove durability across a clean close, not across power loss.

use std::collections::BTreeMap;
use std::path::Path;

use kernel::{
    AdmissionDomainSpec, AdmissionEvent, AdmissionRequest, CommitIntent, DomainSpec, EventKind,
    KernelStore, ObjectRow, RemediationTarget, Sensitivity, SourceClass, TaintClass,
};
use rusqlite::{Connection, OpenFlags};

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "stage1".to_string(),
        operation_key: key.to_string(),
        request_digest: "a".repeat(64),
        actor: "test".to_string(),
        cause: "stage1".to_string(),
    }
}

fn domain(index: usize) -> DomainSpec {
    DomainSpec {
        domain_id: format!("domain-{index}"),
        object_id: format!("object-{index}"),
        name: format!("name-{index}"),
        source_kind: "fixture".to_string(),
        source_id: format!("source-{index}"),
        source_revision: i64::try_from(index).unwrap(),
        sensitivity: Sensitivity::Normal,
    }
}

fn supersede(object_id: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_string()),
        source_class: Some(SourceClass::TrustedLocalCode),
        taint_class: Some(TaintClass::CurrentCode),
        event: AdmissionEvent {
            kind: EventKind::Replace,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "stage1 supersede".to_string(),
        },
    }
}

/// Live domain objects at the tip, keyed by object id.
fn live_domains(store: &KernelStore) -> BTreeMap<String, ObjectRow> {
    let tip = store.tip().unwrap();
    store
        .known_as_of(tip)
        .unwrap()
        .objects
        .into_iter()
        .filter(|object| object.object_kind == "domain")
        .map(|object| (object.object_id.clone(), object))
        .collect()
}

/// Every open acquires a fresh lease, so an advanced epoch proves
/// `KernelStore::open` built a new store rather than handing back the old one.
fn reopen(root: &Path, store: KernelStore) -> KernelStore {
    let epoch_before = store.lease_epoch();
    drop(store);
    let reopened = KernelStore::open(root).unwrap();
    assert!(reopened.lease_epoch() > epoch_before);
    reopened
}

#[test]
fn every_domain_mutation_is_read_back_after_close_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let created = store
        .commit(intent("create"), |envelope| {
            for index in 1..=4 {
                envelope.insert_domain(domain(index))?;
            }
            Ok(String::new())
        })
        .unwrap();
    let mut corrected = domain(2);
    corrected.domain_id = "domain-2-corrected".to_string();
    corrected.object_id = "object-2-corrected".to_string();
    corrected.name = "name-2-corrected".to_string();
    corrected.source_revision = 20;
    let update = store
        .commit(intent("update"), |envelope| {
            envelope.correct_domain("object-2", corrected)?;
            Ok(String::new())
        })
        .unwrap();
    let deleted = store
        .commit(intent("delete"), |envelope| {
            envelope.retire_domain("object-3")?;
            Ok(String::new())
        })
        .unwrap();
    let superseded = store
        .commit(intent("supersede"), |envelope| {
            envelope.supersede_domain(
                supersede("object-1"),
                AdmissionDomainSpec {
                    domain_id: "domain-successor".to_string(),
                    object_id: "object-successor".to_string(),
                    name: "successor".to_string(),
                },
            )?;
            Ok(String::new())
        })
        .unwrap();
    let before = live_domains(&store);

    let store = reopen(directory.path(), store);
    assert_eq!(store.tip().unwrap(), superseded.commit_seq);
    let live = live_domains(&store);
    assert_eq!(live, before);
    assert_eq!(
        live.keys().collect::<Vec<_>>(),
        ["object-2-corrected", "object-4", "object-successor"]
    );

    // An untouched domain keeps its created row.
    let untouched = &live["object-4"];
    assert_eq!(untouched.domain_id, "domain-4");
    assert_eq!(untouched.source_revision, 4);
    assert_eq!(untouched.created_commit_seq, created.commit_seq);
    assert_eq!(untouched.invalidated_commit_seq, None);

    // A correction replaces the original and names its successor.
    assert_eq!(live["object-2-corrected"].source_revision, 20);
    let history = store.object_history_as_of(superseded.commit_seq).unwrap();
    let historical = |object_id: &str| {
        history
            .objects
            .iter()
            .find(|object| object.object_id == object_id)
            .unwrap()
    };
    let original = historical("object-2");
    assert_eq!(original.invalidated_commit_seq, Some(update.commit_seq));
    assert_eq!(
        original.superseded_by.as_deref(),
        Some("object-2-corrected")
    );

    // A supersession invalidates the original and names its successor.
    let replaced = historical("object-1");
    assert_eq!(replaced.invalidated_commit_seq, Some(superseded.commit_seq));
    assert_eq!(replaced.superseded_by.as_deref(), Some("object-successor"));

    // A retirement invalidates without a successor; the snapshot at the create
    // still lists the object.
    let retired = historical("object-3");
    assert_eq!(retired.invalidated_commit_seq, Some(deleted.commit_seq));
    assert_eq!(retired.superseded_by, None);
    let at_create = store.known_as_of(created.commit_seq).unwrap();
    assert!(
        at_create
            .objects
            .iter()
            .any(|object| object.object_id == "object-3")
    );
}

#[derive(serde::Deserialize)]
struct ReplayedChange {
    change_kind: String,
    object: ReplayedObject,
    replaced_object_id: Option<String>,
}

/// Mirrors `ObjectRow`, which derives `Serialize` only.
///
/// `deny_unknown_fields` fails the replay when a payload carries a column this
/// mirror lacks; `replayed_object` destructures `ObjectRow`, so an added field
/// must be added here before the test compiles.
#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReplayedObject {
    object_id: String,
    object_kind: String,
    domain_id: String,
    source_kind: String,
    source_id: String,
    source_revision: i64,
    created_commit_seq: i64,
    invalidated_commit_seq: Option<i64>,
    superseded_by: Option<String>,
    sensitivity: Sensitivity,
}

fn replayed_object(object: ObjectRow) -> ReplayedObject {
    let ObjectRow {
        object_id,
        object_kind,
        domain_id,
        source_kind,
        source_id,
        source_revision,
        created_commit_seq,
        invalidated_commit_seq,
        superseded_by,
        sensitivity,
    } = object;
    ReplayedObject {
        object_id,
        object_kind,
        domain_id,
        source_kind,
        source_id,
        source_revision,
        created_commit_seq,
        invalidated_commit_seq,
        superseded_by,
        sensitivity,
    }
}

#[test]
fn outbox_events_replayed_from_the_first_position_rebuild_the_live_registry() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    store
        .commit(intent("create"), |envelope| {
            envelope.insert_domain(domain(1))?;
            envelope.insert_domain(domain(2))?;
            envelope.insert_domain(domain(3))?;
            Ok(String::new())
        })
        .unwrap();
    let mut corrected = domain(2);
    corrected.domain_id = "domain-2-corrected".to_string();
    corrected.object_id = "object-2-corrected".to_string();
    corrected.source_revision = 20;
    store
        .commit(intent("update"), |envelope| {
            envelope.correct_domain("object-2", corrected)?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("delete"), |envelope| {
            envelope.retire_domain("object-3")?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("supersede"), |envelope| {
            envelope.supersede_domain(
                supersede("object-1"),
                AdmissionDomainSpec {
                    domain_id: "domain-successor".to_string(),
                    object_id: "object-successor".to_string(),
                    name: "successor".to_string(),
                },
            )?;
            Ok(String::new())
        })
        .unwrap();
    // `object-3` is already retired here, so its remediation payload carries `invalidated_commit_seq`.
    store
        .commit(intent("remediate"), |envelope| {
            envelope.remediate_text(
                RemediationTarget::CanonicalDomainName {
                    object_id: "object-successor".to_string(),
                },
                "operator",
                7,
            )?;
            envelope.remediate_text(
                RemediationTarget::CanonicalDomainName {
                    object_id: "object-3".to_string(),
                },
                "operator",
                7,
            )?;
            Ok(String::new())
        })
        .unwrap();

    let connection = Connection::open_with_flags(
        directory.path().join("kernel.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = connection
        .prepare("SELECT payload FROM outbox ORDER BY outbox_position")
        .unwrap();
    let payloads: Vec<Vec<u8>> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut replayed: BTreeMap<String, ReplayedObject> = BTreeMap::new();
    for payload in &payloads {
        let change: ReplayedChange = serde_json::from_slice(payload).unwrap();
        if change.object.object_kind != "domain" {
            continue;
        }
        match change.change_kind.as_str() {
            "insert" => {
                replayed.insert(change.object.object_id.clone(), change.object);
            }
            "correct" | "replace" => {
                replayed.remove(change.replaced_object_id.as_deref().unwrap());
                replayed.insert(change.object.object_id.clone(), change.object);
            }
            "retire" => {
                replayed.remove(&change.object.object_id);
            }
            // Remediation can update invalidated rows; replay removes rows with
            // `invalidated_commit_seq` set.
            "operator_remediation" => {
                if change.object.invalidated_commit_seq.is_some() {
                    replayed.remove(&change.object.object_id);
                } else {
                    replayed.insert(change.object.object_id.clone(), change.object);
                }
            }
            other => panic!("unexpected domain change kind {other}"),
        }
    }

    let live: BTreeMap<String, ReplayedObject> = live_domains(&store)
        .into_values()
        .map(|object| (object.object_id.clone(), replayed_object(object)))
        .collect();
    assert_eq!(replayed, live);
    assert_eq!(
        live.keys().collect::<Vec<_>>(),
        ["object-2-corrected", "object-successor"]
    );
}
