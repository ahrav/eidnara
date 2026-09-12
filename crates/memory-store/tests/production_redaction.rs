use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[path = "support/scan_audit.rs"]
mod scan_audit;

use cache_stability::{CoreState, DurabilityClass, FrozenUnit};
use context_core::redaction::RedactionErrorKind;
use memory_store::{
    AuthoritySeedRow, DURABLE_WRITE_REGISTRY, FacadeMutationOutcome, LineageAnchor,
    LineageConstituent, LineageDescentDisposition, LineageDescentRequest, MemoryStore,
    MemoryStoreError, ModuleMeta, NoteEvaluationInput, NoteInput, NoteTransitionInput,
    NoteWriteInput, StoredCompartment, TailHygieneBaseline,
};
use rusqlite::{Connection, backup::Backup};
use scan_audit::{ScanAuditCounts, scan_audit_counts};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Fixture {
    schema: String,
    content: Vec<ContentCase>,
}

#[derive(Deserialize)]
struct ContentCase {
    input: String,
    stored: String,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/durable-field-policy-v1.json")).unwrap()
}

/// `control` must occur in the stored bytes so an absence assertion cannot pass
/// without scanning them.
fn store_family_bytes(root: &std::path::Path, control: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("memory.sqlite")
            && entry.file_type().unwrap().is_file()
        {
            bytes.extend(std::fs::read(entry.path()).unwrap());
        }
    }
    assert!(!bytes.is_empty());
    assert!(
        bytes
            .windows(control.len())
            .any(|window| window == control.as_bytes()),
        "scan did not observe stored text, so an absence check would be vacuous"
    );
    bytes
}

#[test]
fn active_note_scan_audit_is_atomic_complete_and_opaque() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-scan-audit");
    let store = MemoryStore::open(&descriptor).unwrap();
    store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "plain content",
            surface_condition: Some("password=surface-secret"),
            anchor_block_id: Some("block-1"),
            now_ms: 1,
        })
        .unwrap();

    let note_audit = ScanAuditCounts {
        batches: 1,
        owner_scopes: 1,
        domain_owners: 1,
        field_scans: 3,
        owner_copies: 3,
        detections: 1,
    };
    assert_eq!(scan_audit_counts(temp.path()), note_audit);
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let ids = connection
        .prepare(
            "SELECT b.scan_batch_id,s.scan_id,o.owner_copy_id,o.field_id,s.finding_count
               FROM scan_batches b
               JOIN field_scans s USING(scan_batch_id)
               JOIN scan_owner_copies o USING(scan_id)
              ORDER BY o.field_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        ids.iter().map(|row| row.3.as_str()).collect::<Vec<_>>(),
        vec!["anchor_block_id", "content", "surface_condition"]
    );
    for (batch_id, scan_id, owner_copy_id, _, _) in &ids {
        for id in [batch_id, scan_id, owner_copy_id] {
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(!id.contains("project"));
            assert!(!id.contains("secret"));
        }
    }
    let private_owner_material: Vec<(String, String)> = connection
        .prepare(
            "SELECT scope_key,owner_key FROM scan_owner_scopes
             JOIN scan_domain_owners USING(owner_scope_id)",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!private_owner_material.is_empty());
    for (scope_key, owner_key) in private_owner_material {
        for value in [scope_key, owner_key] {
            assert_eq!(value.len(), 64);
            assert!(value.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(!value.contains("project"));
            assert!(!value.contains("session"));
        }
    }
    assert_eq!(
        ids.iter().map(|row| row.4).collect::<Vec<_>>(),
        vec![0, 0, 1]
    );
    let family = store_family_bytes(temp.path(), "plain content");
    for forbidden in ["surface-secret", "password=surface-secret"] {
        assert!(
            !family
                .windows(forbidden.len())
                .any(|window| window == forbidden.as_bytes())
        );
    }
    // A detection row may carry only identity, ordinal, and bounded classifier metadata.
    // Any new column must be added here deliberately, so a byte offset, span length, or
    // matched text cannot appear under an unanticipated name.
    let columns = connection
        .prepare("PRAGMA table_info(scan_detections)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "scan_id",
            "detection_ordinal",
            "exactness",
            "label_id",
            "span_kind",
            "action",
        ]
    );
    drop(connection);
    drop(store);
    let reopened = MemoryStore::open(&descriptor).unwrap();
    assert_eq!(scan_audit_counts(temp.path()), note_audit);
    drop(reopened);
}

#[test]
fn active_scan_audit_expires_with_its_session_note_owner() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-scan-retention");
    let store = MemoryStore::open(&descriptor).unwrap();
    store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "password=retained-secret",
            surface_condition: None,
            anchor_block_id: None,
            now_ms: 1,
        })
        .unwrap();
    assert_eq!(
        scan_audit_counts(temp.path()),
        ScanAuditCounts {
            batches: 1,
            owner_scopes: 1,
            domain_owners: 1,
            field_scans: 1,
            owner_copies: 1,
            detections: 1,
        }
    );

    store.delete_session("session", "project").unwrap();

    assert_eq!(scan_audit_counts(temp.path()), ScanAuditCounts::EMPTY);

    store
        .append_compartments(
            "source",
            &[StoredCompartment {
                sequence: 1,
                start_message: 1,
                end_message: 3,
                start_message_id: "m1#0".to_string(),
                end_message_id: "m3#0".to_string(),
                content: "copied password=compartment-secret".to_string(),
                ..StoredCompartment::default()
            }],
        )
        .unwrap();
    // The compartment prepares session_id, start_message_id, end_message_id, title, and
    // content; only content carries a detection.
    assert_eq!(
        scan_audit_counts(temp.path()),
        ScanAuditCounts {
            batches: 1,
            owner_scopes: 1,
            domain_owners: 1,
            field_scans: 5,
            owner_copies: 5,
            detections: 1,
        }
    );
}

#[test]
fn active_scan_audit_survives_until_its_last_owner_expires() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-scan-shared-retention");
    let store = MemoryStore::open(&descriptor).unwrap();
    let note_id = store
        .seed_authority_rows(
            "store-uuid",
            "project",
            "notes",
            &[AuthoritySeedRow {
                source_row_id: 1,
                snapshot: json!({
                    "id": 1,
                    "project_path": "project",
                    "session_id": "source-session",
                    "content": "plain note",
                    "status": "ready"
                }),
            }],
        )
        .unwrap()[0];
    store
        .claim_note_delivery("project", "delivery-session", "fingerprint", "pass", 2)
        .unwrap();

    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let batch_id: String = connection
        .query_row(
            "SELECT b.scan_batch_id
               FROM scan_batches b
               JOIN field_scans s USING (scan_batch_id)
               JOIN scan_owner_copies c USING (scan_id)
              WHERE b.owner_kind = 'notes'
              GROUP BY b.scan_batch_id
             HAVING COUNT(DISTINCT c.domain_owner_id) = 2
              LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);

    store.delete_session("delivery-session", "project").unwrap();
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let remaining_owners: i64 = connection
        .query_row(
            "SELECT COUNT(DISTINCT c.domain_owner_id)
               FROM field_scans s
               JOIN scan_owner_copies c USING (scan_id)
              WHERE s.scan_batch_id = ?1",
            [&batch_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining_owners, 1);
    drop(connection);

    assert_eq!(
        store
            .read_project_notes("project", None, &["surfacing"], 10, 0)
            .unwrap()[0]
            .id,
        note_id
    );
    store
        .authority_begin_prepare("store-uuid", "project", "notes")
        .unwrap();
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let retained: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM scan_batches WHERE scan_batch_id = ?1",
            [&batch_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, 0);
}

#[test]
fn lineage_copy_links_source_scans_without_rescanning_and_survives_source_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-lineage-scan-links");
    let store = MemoryStore::open(&descriptor).unwrap();
    store
        .commit(
            "source",
            None,
            &CoreState {
                boundary_id: "password=lineage-secret".to_string(),
                ..CoreState::empty()
            },
            &ModuleMeta {
                initialized: true,
                newest_live_ordinal: 3,
                coverage_ordinal: Some(3),
                ..ModuleMeta::default()
            },
        )
        .unwrap();

    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let source_scan_ids = connection
        .prepare(
            "SELECT DISTINCT copies.scan_id
               FROM scan_owner_copies copies
               JOIN scan_domain_owners owners USING(domain_owner_id)
               JOIN scan_owner_scopes scopes USING(owner_scope_id)
              WHERE scopes.scope_kind='session'",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<BTreeSet<_>>>()
        .unwrap();
    assert!(!source_scan_ids.is_empty());
    // Descent scans the target metadata it writes, so total scans grow; the source's
    // password detection must not be rescanned.
    let secret_scans = |connection: &Connection| -> i64 {
        connection
            .query_row(
                "SELECT COUNT(DISTINCT scan_id) FROM scan_detections
                  WHERE label_id='password'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let secret_scans_before = secret_scans(&connection);
    assert!(secret_scans_before > 0);
    drop(connection);

    let constituents = [LineageConstituent {
        prior_key: "source".to_string(),
        new_key: "target".to_string(),
        epoch: 1,
    }];
    let anchor = LineageAnchor {
        block_id: "summary#1".to_string(),
        message_id: "summary".to_string(),
        content_hash: "abc123".to_string(),
        ordinal: 1,
    };
    let outcome = store
        .descend_lineage(LineageDescentRequest {
            target_key: "target",
            expected_target_row_version: None,
            edge_id: 1,
            prior_key: "source",
            prior_epoch: 0,
            new_epoch: 1,
            constituents: &constituents,
            compaction_observed: true,
            anchor: Some(&anchor),
            now_ms: 2,
        })
        .unwrap();
    assert_eq!(outcome.disposition, LineageDescentDisposition::Descended);

    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    assert_eq!(
        secret_scans(&connection),
        secret_scans_before,
        "lineage copy rescanned the source instead of linking its scans"
    );
    for scan_id in &source_scan_ids {
        let owner_count: i64 = connection
            .query_row(
                "SELECT COUNT(DISTINCT domain_owner_id)
                   FROM scan_owner_copies WHERE scan_id=?1",
                [scan_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner_count, 2, "source scan was not linked to target");
    }
    drop(connection);

    store.delete_session("source", "project").unwrap();
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    for scan_id in source_scan_ids {
        let retained: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM field_scans WHERE scan_id=?1",
                [scan_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1, "target did not retain linked source scan");
    }
}

#[test]
fn sqlite_online_backup_preserves_active_scan_audit_rows() {
    let source = tempfile::tempdir().unwrap();
    let source_descriptor = MemoryStore::test_descriptor(source.path(), "production-scan-backup");
    let store = MemoryStore::open(&source_descriptor).unwrap();
    store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "password=backup-secret",
            surface_condition: Some("plain condition"),
            anchor_block_id: None,
            now_ms: 1,
        })
        .unwrap();
    let expected = scan_audit_counts(source.path());
    drop(store);

    let restored = tempfile::tempdir().unwrap();
    let source_connection = Connection::open(source.path().join("memory.sqlite")).unwrap();
    let mut restored_connection = Connection::open(restored.path().join("memory.sqlite")).unwrap();
    {
        let backup = Backup::new(&source_connection, &mut restored_connection).unwrap();
        backup
            .run_to_completion(64, Duration::from_millis(1), None)
            .unwrap();
    }
    drop(restored_connection);
    drop(source_connection);

    let restored_descriptor =
        MemoryStore::test_descriptor(restored.path(), "production-scan-restore");
    let restored_store = MemoryStore::open(&restored_descriptor).unwrap();
    assert_eq!(scan_audit_counts(restored.path()), expected);
    assert_eq!(
        restored_store
            .read_notes("project", "session", 10, 0)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn concurrent_facade_duplicate_persists_one_active_scan_batch() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-scan-concurrent");
    let store = Arc::new(MemoryStore::open(&descriptor).unwrap());
    let start = Arc::new(Barrier::new(3));
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let store = Arc::clone(&store);
        let start = Arc::clone(&start);
        let invoked = Arc::clone(&invoked);
        workers.push(std::thread::spawn(move || {
            start.wait();
            store
                .with_facade_command(
                    "route",
                    "project",
                    "notes",
                    "scope",
                    "tool",
                    "write",
                    Some("same-command"),
                    |_| {
                        invoked.fetch_add(1, Ordering::SeqCst);
                        Ok(br#"{"result":"plain"}"#.to_vec())
                    },
                )
                .unwrap()
        }));
    }
    start.wait();
    let outcomes = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(invoked.load(Ordering::SeqCst), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, FacadeMutationOutcome::Applied(_)))
            .count(),
        1
    );
    assert_eq!(scan_audit_counts(temp.path()).batches, 1);
}

/// `every_durable_write_family_is_declared_once_with_a_distinct_owner_kind` owns declaration completeness.
#[test]
fn durable_write_registry_references_real_bindings_and_checked_tests() {
    let store_source = include_str!("../src/lib.rs");
    assert!(!store_source.contains("PreparedWrite::new(\""));

    // The module half of `<module>::<fn>` must resolve to one of these sources, so a
    // same-named function in an unrelated file cannot satisfy the entry.
    let sources: BTreeMap<&str, &str> = BTreeMap::from([
        (
            "production_redaction",
            include_str!("production_redaction.rs"),
        ),
        ("lib", store_source),
        // Kernel oracles live in the extracted crate; the registry still names them.
        (
            "kernel_redaction",
            include_str!("../../kernel/tests/kernel_redaction.rs"),
        ),
        (
            "kernel_envelope",
            include_str!("../../kernel/tests/kernel_envelope.rs"),
        ),
        (
            "kernel_outbox",
            include_str!("../../kernel/tests/kernel_outbox.rs"),
        ),
        (
            "kernel_schema",
            include_str!("../../kernel/tests/kernel_schema.rs"),
        ),
    ]);
    for entry in DURABLE_WRITE_REGISTRY {
        let (module, test_name) = entry
            .test
            .rsplit_once("::")
            .unwrap_or_else(|| panic!("{:?} test {} is not module::fn", entry.family, entry.test));
        let source = sources.get(module).unwrap_or_else(|| {
            panic!(
                "{:?} references module {module}, which is not a checked test source",
                entry.family
            )
        });
        let signature = format!("fn {test_name}(");
        let position = source.find(&signature).unwrap_or_else(|| {
            panic!(
                "{:?} references missing function {}",
                entry.family, entry.test
            )
        });
        let preceding = &source[position.saturating_sub(256)..position];
        let attributes = preceding
            .rsplit_once("#[test]")
            .map(|(_, attributes)| attributes)
            .unwrap_or_else(|| {
                panic!(
                    "{:?} references {} which is not a #[test] function",
                    entry.family, entry.test
                )
            });
        assert!(
            !attributes.contains('}') && !attributes.contains("fn "),
            "{:?} references {} but the nearest #[test] belongs to another item",
            entry.family,
            entry.test
        );
        assert!(
            !attributes.contains("ignore"),
            "{:?} references {} which is ignored and never runs",
            entry.family,
            entry.test
        );
    }
}

/// The `meta` column's byte and receipt contract: clean `meta` is stored as its own
/// serialization byte for byte with no detection recorded; a secret planted in a nested
/// map's value is substituted and recorded as exactly one detection on the `meta` scan; a
/// secret planted in a nested map's key is refused with no row and no receipt, even when an
/// earlier value in the same document carried a detection.
#[test]
fn cache_state_meta_is_stored_byte_identical_when_clean_and_scanned_to_every_nested_key() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-meta");
    let store = MemoryStore::open(&descriptor).unwrap();
    let stored_meta = |session: &str| -> String {
        let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT meta FROM cache_state WHERE session_id = ?1",
                [session],
                |row| row.get(0),
            )
            .unwrap()
    };
    let meta_detections = |connection: &Connection| -> i64 {
        connection
            .query_row(
                "SELECT COUNT(*) FROM scan_detections d JOIN field_scans s ON s.scan_id = d.scan_id \
                 JOIN scan_owner_copies o ON o.scan_id = s.scan_id WHERE o.field_id = 'meta'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };

    let mut clean = ModuleMeta::default();
    clean.block_identity_by_mid.insert(
        "mid-1".to_string(),
        vec![memory_store::BlockIdentity {
            kind_tag: "text".to_string(),
            byte_fingerprint: "fp".to_string(),
        }],
    );
    clean.last_render_config = "render-v1".to_string();
    store
        .commit("clean", None, &CoreState::empty(), &clean)
        .unwrap();
    assert_eq!(
        stored_meta("clean"),
        serde_json::to_string(&clean).unwrap(),
        "clean meta is stored as its serialization, byte for byte"
    );
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    assert_eq!(
        meta_detections(&connection),
        0,
        "clean meta records no detection"
    );
    drop(connection);

    // A secret under a nested map value is substituted and recorded.
    let mut planted = ModuleMeta::default();
    planted.block_identity_by_mid.insert(
        "mid-1".to_string(),
        vec![memory_store::BlockIdentity {
            kind_tag: "password=planted-secret".to_string(),
            byte_fingerprint: "fp".to_string(),
        }],
    );
    store
        .commit("planted", None, &CoreState::empty(), &planted)
        .unwrap();
    let stored = stored_meta("planted");
    assert!(!stored.contains("planted-secret"), "{stored}");
    assert!(stored.contains("password=<REDACTED:password>"), "{stored}");
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    assert_eq!(
        meta_detections(&connection),
        1,
        "the substitution left exactly one detection on the meta scan"
    );
    drop(connection);

    let mut keyed = ModuleMeta::default();
    keyed.block_identity_by_mid.insert(
        "a-mid".to_string(),
        vec![memory_store::BlockIdentity {
            kind_tag: "password=earlier-value".to_string(),
            byte_fingerprint: "fp".to_string(),
        }],
    );
    keyed.block_identity_by_mid.insert(
        "password=key-secret".to_string(),
        vec![memory_store::BlockIdentity {
            kind_tag: "text".to_string(),
            byte_fingerprint: "fp".to_string(),
        }],
    );
    let refused = store
        .commit("keyed", None, &CoreState::empty(), &keyed)
        .unwrap_err();
    assert!(
        matches!(refused, memory_store::MemoryStoreError::Redaction(_)),
        "{refused:?}"
    );
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let keyed_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM cache_state WHERE session_id = 'keyed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(keyed_rows, 0, "a refused meta stores nothing");
    assert_eq!(
        meta_detections(&connection),
        1,
        "a refused meta records no receipt; only the planted session's detection remains"
    );
}

#[test]
fn cache_state_redacts_payloads_preserves_existing_ids_and_rejects_integrity() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-cache-state");
    let store = MemoryStore::open(&descriptor).unwrap();
    let core = CoreState {
        version: 1,
        boundary_id: "boundary".to_string(),
        frozen_units: vec![FrozenUnit {
            key: "unit".to_string(),
            kind: "drop".to_string(),
            frozen_payload: "password=payload-secret".to_string(),
            durability_class: DurabilityClass::Episode,
            reset_rule: "run_started".to_string(),
        }],
        ..CoreState::empty()
    };
    store
        .commit("session", None, &core, &ModuleMeta::default())
        .unwrap();
    let loaded = store.load("session").unwrap();
    assert_eq!(
        loaded.core.frozen_units[0].frozen_payload,
        "password=<REDACTED:password>"
    );

    // Redacting a persisted boundary ID would collapse distinct legacy boundaries onto one
    // placeholder, so an existing row's boundary must round-trip verbatim.
    let mut existing_identity = core;
    existing_identity.boundary_id = "password=legacy-boundary".to_string();
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO cache_state(session_id, row_version, core_state, meta)
             VALUES (?1, 0, ?2, ?3)",
            rusqlite::params![
                "existing-identity",
                serde_json::to_string(&existing_identity).unwrap(),
                serde_json::to_string(&ModuleMeta::default()).unwrap()
            ],
        )
        .unwrap();
    drop(connection);
    store
        .commit(
            "existing-identity",
            Some(0),
            &existing_identity,
            &ModuleMeta::default(),
        )
        .unwrap();
    assert_eq!(
        store.load("existing-identity").unwrap().core.boundary_id,
        "password=legacy-boundary"
    );

    let error = store
        .commit(
            "password=new-session",
            None,
            &CoreState::empty(),
            &ModuleMeta::default(),
        )
        .unwrap_err();
    assert!(
        matches!(error, memory_store::MemoryStoreError::Redaction(_)),
        "{error:?}"
    );
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO cache_state(session_id, row_version, core_state, meta)
             VALUES (?1, 0, ?2, ?3)",
            rusqlite::params![
                "password=legacy-session",
                serde_json::to_string(&CoreState::empty()).unwrap(),
                serde_json::to_string(&ModuleMeta::default()).unwrap()
            ],
        )
        .unwrap();
    drop(connection);
    store
        .commit(
            "password=legacy-session",
            Some(0),
            &CoreState::empty(),
            &ModuleMeta::default(),
        )
        .unwrap();

    let meta = ModuleMeta {
        tail_hygiene_baseline: Some(TailHygieneBaseline {
            content_signature: "password=signature-secret".to_string(),
            ..TailHygieneBaseline::default()
        }),
        ..ModuleMeta::default()
    };
    let error = store
        .commit("rejected", None, &CoreState::empty(), &meta)
        .unwrap_err();
    assert!(matches!(
        error,
        memory_store::MemoryStoreError::Redaction(_)
    ));
    assert!(!error.to_string().contains("signature-secret"));
    assert!(store.load("rejected").unwrap().row_version.is_none());
}

#[test]
fn transform_diagnostics_redact_before_persistence() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-diagnostic");
    let store = MemoryStore::open(&descriptor).unwrap();
    store
        .trace_pass_rejected("session", "password=trace-secret", 1)
        .unwrap();
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let stored: String = connection
        .query_row("SELECT last_reject_error FROM pass_trace", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored, "password=<REDACTED:password>");
}

#[test]
fn authority_routes_reject_new_secret_identities_and_preserve_exact_existing_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor =
        MemoryStore::test_descriptor(temp.path(), "production-redaction-authority-route");
    let store = MemoryStore::open(&descriptor).unwrap();

    let error = store
        .bind_authority_route("store", "project", "password=new-route")
        .unwrap_err();
    assert!(matches!(
        error,
        memory_store::MemoryStoreError::Redaction(_)
    ));

    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO authority(context_store_uuid, project, domain, state)
             VALUES (?1, ?2, 'notes', 'TS')",
            ["password=legacy-store", "password=legacy-project"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO authority_route_bindings(route_project_root, context_store_uuid, project)
             VALUES (?1, ?2, ?3)",
            [
                "password=legacy-route",
                "password=legacy-store",
                "password=legacy-project",
            ],
        )
        .unwrap();
    drop(connection);

    store
        .bind_authority_route(
            "password=legacy-store",
            "password=legacy-project",
            "password=legacy-route",
        )
        .unwrap();
}

#[test]
fn mural_artifacts_reject_secret_bytes_hashes_and_new_identity() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-mural");
    let store = MemoryStore::open(&descriptor).unwrap();

    for (project, data, hash) in [
        (
            "password=project-secret",
            "data:image/png;base64,YQ==",
            "hash",
        ),
        ("project", "password=mural-secret", "hash"),
        (
            "project",
            "data:image/png;base64,YQ==",
            "password=hash-secret",
        ),
    ] {
        let error = store
            .upsert_project_mural_artifact(project, data.as_bytes(), hash, 1)
            .unwrap_err();
        assert!(matches!(
            error,
            memory_store::MemoryStoreError::Redaction(_)
        ));
        assert!(!error.to_string().contains("secret"));
    }
    let persisted: i64 = Connection::open(temp.path().join("memory.sqlite"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM project_mural_artifacts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        persisted, 0,
        "a rejected artifact was stored under some project key"
    );
    assert!(
        store
            .load_project_mural_artifact("project")
            .unwrap()
            .is_none()
    );
}

#[test]
fn workspace_member_seed_redacts_share_categories_and_rejects_secret_identities() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-workspace");
    let store = MemoryStore::open(&descriptor).unwrap();

    store
        .seed_workspace_member(
            "workspace",
            "project",
            r#"["CONSTRAINTS","password=share-secret"]"#,
        )
        .unwrap();
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let (share_categories, members): (String, i64) = connection
        .query_row(
            "SELECT share_categories,
                    (SELECT COUNT(*) FROM workspace_members)
               FROM workspaces WHERE name = 'workspace'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(share_categories.contains("CONSTRAINTS"));
    assert!(
        share_categories.contains("<REDACTED:"),
        "{share_categories}"
    );
    assert!(!share_categories.contains("share-secret"));
    assert_eq!(members, 1);
    let seeded_audit = ScanAuditCounts {
        batches: 1,
        owner_scopes: 1,
        domain_owners: 1,
        field_scans: 3,
        owner_copies: 3,
        detections: 1,
    };
    assert_eq!(scan_audit_counts(temp.path()), seeded_audit);
    drop(connection);

    for (workspace, project_path) in [
        ("password=workspace-secret", "project"),
        ("other-workspace", "password=path-secret"),
    ] {
        let error = store
            .seed_workspace_member(workspace, project_path, "[]")
            .unwrap_err();
        assert!(
            matches!(
                error,
                MemoryStoreError::Redaction(RedactionErrorKind::SecretDetected)
            ),
            "{error:?}"
        );
        assert!(!error.to_string().contains("secret"));
    }
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let (workspaces, members): (i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM workspaces),
                    (SELECT COUNT(*) FROM workspace_members)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((workspaces, members), (1, 1));
    assert_eq!(scan_audit_counts(temp.path()), seeded_audit);
    let family = store_family_bytes(temp.path(), "CONSTRAINTS");
    for forbidden in ["share-secret", "workspace-secret", "path-secret"] {
        assert!(
            !family
                .windows(forbidden.len())
                .any(|window| window == forbidden.as_bytes())
        );
    }
}

#[test]
fn authority_creation_and_checksums_reject_secret_material() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-authority");
    let store = MemoryStore::open(&descriptor).unwrap();

    let error = store
        .authority_begin_prepare("password=store-secret", "project", "notes")
        .unwrap_err();
    assert!(
        matches!(error, memory_store::MemoryStoreError::Redaction(_)),
        "{error:?}"
    );
    assert!(
        store
            .authority_status("password=store-secret", "project", "notes")
            .unwrap()
            .is_none()
    );

    let preparing = store
        .authority_begin_prepare("0123456789abcdef0123456789abcdef", "project", "notes")
        .unwrap();
    let error = store
        .authority_verify_prepare(
            "0123456789abcdef0123456789abcdef",
            "project",
            "notes",
            preparing.generation,
            "password=checksum-secret",
            "clean-checksum",
        )
        .unwrap_err();
    assert!(matches!(
        error,
        memory_store::MemoryStoreError::Redaction(_)
    ));
    assert!(!error.to_string().contains("checksum-secret"));

    let ready = store
        .authority_begin_prepare("legacy-store", "legacy-project", "notes")
        .unwrap();
    store
        .authority_finish_prepare(
            "legacy-store",
            "legacy-project",
            "notes",
            ready.generation,
            "hash",
            "hash",
            true,
        )
        .unwrap();
    store
        .authority_begin_drain(
            "legacy-store",
            "legacy-project",
            "notes",
            "clean-lease",
            100,
            0,
        )
        .unwrap();
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE authority SET coordinator_lease = 'password=legacy-lease'
              WHERE context_store_uuid = 'legacy-store' AND project = 'legacy-project'
                AND domain = 'notes'",
            [],
        )
        .unwrap();
    drop(connection);
    store
        .authority_begin_drain(
            "legacy-store",
            "legacy-project",
            "notes",
            "password=legacy-lease",
            200,
            0,
        )
        .unwrap();
    let error = store
        .authority_begin_drain(
            "legacy-store",
            "legacy-project",
            "notes",
            "password=new-lease",
            300,
            0,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        memory_store::MemoryStoreError::Redaction(_)
    ));
}

#[test]
fn note_fields_follow_content_and_integrity_policy() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-note-fields");
    let store = MemoryStore::open(&descriptor).unwrap();
    let note = store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "ordinary",
            surface_condition: Some("password=condition-secret"),
            anchor_block_id: Some("anchor"),
            now_ms: 1,
        })
        .unwrap();
    assert_eq!(
        note.surface_condition.as_deref(),
        Some("password=<REDACTED:password>")
    );

    let error = store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "ordinary",
            surface_condition: None,
            anchor_block_id: Some("password=anchor-secret"),
            now_ms: 2,
        })
        .unwrap_err();
    assert!(matches!(
        error,
        memory_store::MemoryStoreError::Redaction(_)
    ));
    assert!(!error.to_string().contains("anchor-secret"));

    let error = store
        .insert_project_note(NoteWriteInput {
            project_path: "project",
            route_project_root: None,
            session_id: Some("session"),
            content: "ordinary",
            surface_condition: None,
            anchor_block_id: None,
            anchor_ordinal: None,
            compiled_provider: Some("password=provider-secret"),
            compiled_config: None,
            compiled_at: None,
            compile_status: None,
            now_ms: 3,
        })
        .unwrap_err();
    assert!(matches!(
        error,
        memory_store::MemoryStoreError::Redaction(_)
    ));
    assert!(!error.to_string().contains("provider-secret"));
}

#[test]
fn active_note_writes_redact_before_persistence() {
    let fixture = fixture();
    assert_eq!(fixture.schema, "eidnara.durable-field-policy/v1");
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-note");
    let store = MemoryStore::open(&descriptor).unwrap();

    for (index, case) in fixture.content.iter().enumerate() {
        let session = format!("session-{index}");
        store
            .insert_note(NoteInput {
                project_path: "project",
                route_project_root: None,
                session_id: &session,
                content: &case.input,
                surface_condition: None,
                anchor_block_id: None,
                now_ms: index as i64,
            })
            .unwrap();
        let notes = store.read_notes("project", &session, 10, 0).unwrap();
        assert_eq!(notes[0].content, case.stored);
    }
}

#[test]
fn a_provider_rule_detection_persists_its_label_in_scan_detections() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-provider-label");
    let store = MemoryStore::open(&descriptor).unwrap();
    // Assembled at runtime so the fixture carries no credential-shaped literal for a
    // repository secret scanner to flag; the value the redactor sees is unchanged.
    let credential = format!(
        "xoxb-{}-{}-{}",
        "123456789012", "1234567890123", "abcdefghijklmnopqrstuvwx"
    );
    let content = format!("deploy token {credential} rotated");

    store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: &content,
            surface_condition: None,
            anchor_block_id: None,
            now_ms: 1,
        })
        .unwrap();

    let notes = store.read_notes("project", "session", 10, 0).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(
        !notes[0].content.contains(&credential),
        "the credential must not persist: {}",
        notes[0].content
    );
    assert!(notes[0].content.contains("<SLACK_TOKEN_REDACTED>"));

    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let labels = connection
        .prepare("SELECT DISTINCT label_id FROM scan_detections ORDER BY label_id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(labels, vec!["slack_token".to_string()]);
}

#[test]
fn transaction_produced_facade_text_is_redacted_and_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-facade");
    let store = MemoryStore::open(&descriptor).unwrap();
    let outcome = store
        .with_facade_command(
            "route",
            "project",
            "notes",
            "scope",
            "tool",
            "write",
            Some("command-1"),
            |_| Ok(br#"{"result":"password=hunter-two"}"#.to_vec()),
        )
        .unwrap();
    let FacadeMutationOutcome::Applied(response) = outcome else {
        panic!("first facade command must apply");
    };
    assert_eq!(
        std::str::from_utf8(&response).unwrap(),
        r#"{"result":"password=<REDACTED:password>"}"#
    );
    let persisted_response: Vec<u8> = Connection::open(temp.path().join("memory.sqlite"))
        .unwrap()
        .query_row(
            "SELECT response_json FROM facade_mutation_ledger WHERE command_id='command-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_response, response);
    let committed_audit = scan_audit_counts(temp.path());
    let replay = store
        .with_facade_command(
            "route",
            "project",
            "notes",
            "scope",
            "tool",
            "write",
            Some("command-1"),
            |_| panic!("replay must not invoke mutation"),
        )
        .unwrap();
    assert_eq!(replay, FacadeMutationOutcome::Duplicate(response));
    assert_eq!(scan_audit_counts(temp.path()), committed_audit);

    let oversized = store.with_facade_command(
        "route",
        "project",
        "notes",
        "scope",
        "tool",
        "write",
        Some("command-2"),
        |_| Ok(vec![b'x'; 512 * 1024 + 1]),
    );
    assert!(matches!(
        oversized,
        Err(memory_store::MemoryStoreError::Redaction(
            context_core::redaction::RedactionErrorKind::InputLimit
        ))
    ));
    assert_eq!(scan_audit_counts(temp.path()), committed_audit);

    let mut invoked = false;
    let error = store
        .with_facade_command(
            "route",
            "project",
            "notes",
            "scope",
            "password=tool-secret",
            "write",
            None,
            |_| {
                invoked = true;
                Ok(Vec::new())
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        memory_store::MemoryStoreError::Redaction(_)
    ));
    assert!(!invoked);

    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO facade_mutation_ledger
                 (identity_scope, tool, action, command_id, response_json, created_at_ms)
             VALUES ('legacy-scope', 'password=legacy-tool', 'password=legacy-action',
                     'legacy-command', X'7B7D', 1)",
            [],
        )
        .unwrap();
    drop(connection);
    let replay = store
        .with_facade_command(
            "route",
            "project",
            "notes",
            "legacy-scope",
            "password=legacy-tool",
            "password=legacy-action",
            Some("legacy-command"),
            |_| panic!("exact replay must not invoke mutation"),
        )
        .unwrap();
    assert_eq!(replay, FacadeMutationOutcome::Duplicate(b"{}".to_vec()));
    let existing = store
        .with_facade_command(
            "route",
            "project",
            "notes",
            "legacy-scope",
            "password=legacy-tool",
            "password=legacy-action",
            Some("next-command"),
            |_| Ok(b"{}".to_vec()),
        )
        .unwrap();
    assert_eq!(existing, FacadeMutationOutcome::Applied(b"{}".to_vec()));
}

/// The `notes_facade_authority_*` triggers abort a facade write whenever the caller's
/// route resolves to a `notes` authority in any state other than `MODULE`.
#[test]
fn facade_note_mutations_abort_unless_the_notes_authority_is_module() {
    const STORE_UUID: &str = "0123456789abcdef0123456789abcdef";
    const PROJECT: &str = "project";
    const ROUTE: &str = "route";
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-facade-authority");
    let store = MemoryStore::open(&descriptor).unwrap();
    store
        .bind_authority_route(STORE_UUID, PROJECT, ROUTE)
        .unwrap();
    let preparing = store
        .authority_begin_prepare(STORE_UUID, PROJECT, "notes")
        .unwrap();
    assert_eq!(preparing.state, "PREPARING");

    let facade_insert = |command: &str| {
        store.with_facade_command(
            ROUTE,
            PROJECT,
            "notes",
            "scope",
            "tool",
            "insert",
            Some(command),
            |txn| {
                txn.insert_note(NoteInput {
                    project_path: PROJECT,
                    route_project_root: None,
                    session_id: "session",
                    content: "facade note",
                    surface_condition: None,
                    anchor_block_id: None,
                    now_ms: 1,
                })
                .map(|note| note.id.to_string().into_bytes())
            },
        )
    };
    let read = || store.read_notes(PROJECT, "session", 10, 0).unwrap();

    let audit_before = scan_audit_counts(temp.path());
    let notes_before = read();
    let error = facade_insert("insert-while-preparing").unwrap_err();
    assert!(error.to_string().contains("authority_draining"), "{error}");
    assert_eq!(read(), notes_before);
    assert_eq!(scan_audit_counts(temp.path()), audit_before);

    let module = store
        .authority_finish_prepare(
            STORE_UUID,
            PROJECT,
            "notes",
            preparing.generation,
            "h",
            "h",
            true,
        )
        .unwrap();
    assert_eq!(module.state, "MODULE");
    let FacadeMutationOutcome::Applied(response) = facade_insert("insert-while-module").unwrap()
    else {
        panic!("a fresh command id must apply");
    };
    let note_id: i64 = std::str::from_utf8(&response).unwrap().parse().unwrap();
    let notes_before = read();
    assert_eq!(notes_before.len(), 1);
    assert_eq!(notes_before[0].id, note_id);

    let draining = store
        .authority_begin_drain(STORE_UUID, PROJECT, "notes", "lease", 100, 0)
        .unwrap();
    assert_eq!(draining.state, "DRAINING");
    let audit_before = scan_audit_counts(temp.path());
    let status_version = notes_before[0].status_version;
    let cases: [(&str, Result<FacadeMutationOutcome, MemoryStoreError>); 3] = [
        ("insert", facade_insert("insert-while-draining")),
        (
            "update",
            store.with_facade_command(
                ROUTE,
                PROJECT,
                "notes",
                "scope",
                "tool",
                "update",
                Some("update-while-draining"),
                |txn| {
                    txn.update_note_cas(
                        PROJECT,
                        note_id,
                        "active",
                        status_version,
                        Some("changed"),
                        None,
                        None,
                        5,
                    )
                    .map(|_| b"{}".to_vec())
                },
            ),
        ),
        (
            "dismiss",
            store.with_facade_command(
                ROUTE,
                PROJECT,
                "notes",
                "scope",
                "tool",
                "dismiss",
                Some("dismiss-while-draining"),
                |txn| {
                    txn.dismiss_note(PROJECT, "session", note_id, None, 6)
                        .map(|_| b"{}".to_vec())
                },
            ),
        ),
    ];
    for (label, result) in cases {
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("authority_draining"),
            "{label}: {error}"
        );
    }
    assert_eq!(read(), notes_before);
    assert_eq!(scan_audit_counts(temp.path()), audit_before);
}

#[test]
fn new_idempotency_identities_reject_without_substitution_or_collapse() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-identity");
    let store = MemoryStore::open(&descriptor).unwrap();
    let mut accepted_invocations = 0;
    for command_id in ["clean-command-one", "clean-command-two"] {
        let outcome = store
            .with_facade_command(
                "route",
                "project",
                "notes",
                "scope",
                "tool",
                "write",
                Some(command_id),
                |_| {
                    accepted_invocations += 1;
                    Ok(command_id.as_bytes().to_vec())
                },
            )
            .unwrap();
        assert!(matches!(outcome, FacadeMutationOutcome::Applied(_)));
    }
    assert_eq!(accepted_invocations, 2);
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    let accepted: (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),COUNT(DISTINCT command_id)
               FROM facade_mutation_ledger
              WHERE command_id IN ('clean-command-one','clean-command-two')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(accepted, (2, 2));
    drop(connection);

    let mut invoked = false;
    for command_id in ["password=first-identity", "password=second-identity"] {
        let result = store.with_facade_command(
            "route",
            "project",
            "notes",
            "scope",
            "tool",
            "write",
            Some(command_id),
            |_| {
                invoked = true;
                Ok(Vec::new())
            },
        );
        let diagnostic = result.unwrap_err().to_string();
        assert!(!diagnostic.contains(command_id));
        assert!(!diagnostic.contains("first-identity"));
        assert!(!diagnostic.contains("second-identity"));
    }
    assert!(!invoked);
    let connection = Connection::open(temp.path().join("memory.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM facade_mutation_ledger", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        2
    );
}

#[test]
fn compartment_content_redacts_and_new_message_identities_reject() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-compartment");
    let store = MemoryStore::open(&descriptor).unwrap();
    let compartment = StoredCompartment {
        sequence: 1,
        start_message: 1,
        end_message: 2,
        start_message_id: "message-1".to_string(),
        end_message_id: "message-2".to_string(),
        title: "password=title-secret".to_string(),
        content: "password=content-secret".to_string(),
        p1: Some("password=p1-secret".to_string()),
        ..StoredCompartment::default()
    };

    store
        .replace_compartments("session", std::slice::from_ref(&compartment))
        .unwrap();
    let stored = store.load_compartments("session").unwrap();
    assert_eq!(stored[0].title, "password=<REDACTED:password>");
    assert_eq!(stored[0].content, "password=<REDACTED:password>");
    assert_eq!(
        stored[0].p1.as_deref(),
        Some("password=<REDACTED:password>")
    );

    // A non-overlapping range exercises redaction validation instead of overlap validation.
    let rejected = StoredCompartment {
        sequence: 2,
        start_message: 3,
        end_message: 4,
        start_message_id: "password=identity-secret".to_string(),
        end_message_id: "message-4".to_string(),
        ..compartment
    };
    let error = store
        .append_compartments("session", &[rejected])
        .unwrap_err();
    assert!(
        matches!(
            error,
            MemoryStoreError::Redaction(RedactionErrorKind::SecretDetected)
        ),
        "{error:?}"
    );
    assert!(!error.to_string().contains("identity-secret"));
    assert_eq!(store.load_compartments("session").unwrap().len(), 1);
}

#[test]
fn note_transition_content_redacts_and_compiled_artifacts_reject() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(temp.path(), "production-redaction-note-state");
    let store = MemoryStore::open(&descriptor).unwrap();
    let note = store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "ordinary note",
            surface_condition: None,
            anchor_block_id: None,
            now_ms: 1,
        })
        .unwrap();
    let applied = store
        .transition_note(NoteTransitionInput {
            project_path: "project",
            note_id: note.id,
            from_status: "active",
            source_revision: note.status_version,
            to_status: "dismissed",
            result: Some("password=resolution-secret"),
            now_ms: 2,
        })
        .unwrap();
    let memory_store::NoteCasOutcome::Applied(applied) = applied else {
        panic!("note transition must apply");
    };
    assert_eq!(
        applied.dismissal_resolution.as_deref(),
        Some("password=<REDACTED:password>")
    );

    let pending = store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "smart note",
            surface_condition: Some("always"),
            anchor_block_id: None,
            now_ms: 3,
        })
        .unwrap();
    let error = store
        .write_note_evaluation(NoteEvaluationInput {
            project_path: "project",
            note_id: pending.id,
            source_revision: pending.status_version,
            verdict: false,
            compiled_check: Some("password=compiled-secret"),
            manifest_json: None,
            check_hash: None,
            next_due_at: None,
            now_ms: 4,
        })
        .unwrap_err();
    assert!(!error.to_string().contains("compiled-secret"));
}
