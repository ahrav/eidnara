//! Canonical state survives close and reopen, and the outbox replays into the
//! same registry.

#![cfg(feature = "test-support")]

use std::collections::BTreeMap;
use std::path::Path;

use kernel::{
    AdmissionDomainSpec, AdmissionEvent, AdmissionRequest, CommitIntent, DomainSpec, EventKind,
    KernelStore, ObjectRow, Sensitivity, SourceClass, TaintClass,
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

fn reopen(root: &Path, store: KernelStore) -> KernelStore {
    drop(store);
    KernelStore::open(root).unwrap()
}

#[test]
fn a_created_domain_is_read_back_after_close_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let receipt = store
        .commit(intent("create"), |envelope| {
            envelope.insert_domain(domain(1))?;
            Ok(String::new())
        })
        .unwrap();
    let before = live_domains(&store);

    let store = reopen(directory.path(), store);
    assert_eq!(store.tip().unwrap(), receipt.commit_seq);
    let after = live_domains(&store);
    assert_eq!(after, before);
    let object = &after["object-1"];
    assert_eq!(object.domain_id, "domain-1");
    assert_eq!(object.source_revision, 1);
    assert_eq!(object.created_commit_seq, receipt.commit_seq);
    assert_eq!(object.invalidated_commit_seq, None);
}

#[test]
fn an_updated_domain_shows_the_correction_after_close_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    store
        .commit(intent("create"), |envelope| {
            envelope.insert_domain(domain(1))?;
            Ok(String::new())
        })
        .unwrap();
    let mut corrected = domain(1);
    corrected.domain_id = "domain-1-corrected".to_string();
    corrected.object_id = "object-1-corrected".to_string();
    corrected.name = "name-1-corrected".to_string();
    corrected.source_revision = 2;
    let update = store
        .commit(intent("update"), |envelope| {
            envelope.correct_domain("object-1", corrected)?;
            Ok(String::new())
        })
        .unwrap();

    let store = reopen(directory.path(), store);
    let live = live_domains(&store);
    assert_eq!(live.keys().collect::<Vec<_>>(), ["object-1-corrected"]);
    assert_eq!(live["object-1-corrected"].source_revision, 2);
    let history = store.object_history_as_of(update.commit_seq).unwrap();
    let original = history
        .objects
        .iter()
        .find(|object| object.object_id == "object-1")
        .unwrap();
    assert_eq!(original.invalidated_commit_seq, Some(update.commit_seq));
    assert_eq!(
        original.superseded_by.as_deref(),
        Some("object-1-corrected")
    );
}

#[test]
fn a_superseded_domain_shows_its_successor_after_close_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    store
        .commit(intent("create"), |envelope| {
            envelope.insert_domain(domain(1))?;
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

    let store = reopen(directory.path(), store);
    let live = live_domains(&store);
    assert_eq!(live.keys().collect::<Vec<_>>(), ["object-successor"]);
    let history = store.object_history_as_of(superseded.commit_seq).unwrap();
    let original = history
        .objects
        .iter()
        .find(|object| object.object_id == "object-1")
        .unwrap();
    assert_eq!(original.invalidated_commit_seq, Some(superseded.commit_seq));
    assert_eq!(original.superseded_by.as_deref(), Some("object-successor"));
}

#[test]
fn a_deleted_domain_stays_absent_after_close_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    store
        .commit(intent("create"), |envelope| {
            envelope.insert_domain(domain(1))?;
            envelope.insert_domain(domain(2))?;
            Ok(String::new())
        })
        .unwrap();
    let deleted = store
        .commit(intent("delete"), |envelope| {
            envelope.retire_domain("object-1")?;
            Ok(String::new())
        })
        .unwrap();

    let store = reopen(directory.path(), store);
    let live = live_domains(&store);
    assert_eq!(live.keys().collect::<Vec<_>>(), ["object-2"]);
    let history = store.object_history_as_of(deleted.commit_seq).unwrap();
    let retired = history
        .objects
        .iter()
        .find(|object| object.object_id == "object-1")
        .unwrap();
    assert_eq!(retired.invalidated_commit_seq, Some(deleted.commit_seq));
    assert_eq!(retired.superseded_by, None);
    // The delete appended a commit; the snapshot before it still lists the object.
    let before = store.known_as_of(deleted.commit_seq - 1).unwrap();
    assert!(
        before
            .objects
            .iter()
            .any(|object| object.object_id == "object-1")
    );
}

/// One outbox row as a consumer with an empty checkpoint reads it.
#[derive(serde::Deserialize)]
struct ReplayedChange {
    change_kind: String,
    object: ReplayedObject,
    replaced_object_id: Option<String>,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
struct ReplayedObject {
    object_id: String,
    object_kind: String,
    domain_id: String,
    source_revision: i64,
}

#[test]
fn outbox_events_replayed_from_an_empty_checkpoint_rebuild_the_live_registry() {
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

    // Replay from position zero, in position order, folding each change into a registry.
    let connection = Connection::open_with_flags(
        directory.path().join("kernel.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = connection
        .prepare("SELECT payload FROM outbox WHERE outbox_position > 0 ORDER BY outbox_position")
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
            other => panic!("unexpected domain change kind {other}"),
        }
    }

    let live: BTreeMap<String, ReplayedObject> = live_domains(&store)
        .into_values()
        .map(|object| {
            (
                object.object_id.clone(),
                ReplayedObject {
                    object_id: object.object_id,
                    object_kind: object.object_kind,
                    domain_id: object.domain_id,
                    source_revision: object.source_revision,
                },
            )
        })
        .collect();
    assert_eq!(replayed, live);
    assert_eq!(
        live.keys().collect::<Vec<_>>(),
        ["object-2-corrected", "object-successor"]
    );
}
