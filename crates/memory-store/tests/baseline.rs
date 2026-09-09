//! The memory store's on-disk identity: a fresh open lays down exactly the
//! baseline, a reopen accepts it, and any other baseline is refused.

use std::collections::BTreeSet;

use memory_store::{MemoryStore, MemoryStoreError, NoteCasOutcome, NoteInput};
use rusqlite::Connection;
use storage::{INFRASTRUCTURE_TABLES, STORE_BASELINE, StoreError, schema_inventory};

/// Every object the two baselines create, by kind and name, excluding SQLite's own
/// `sqlite_sequence` and `sqlite_autoindex_*`. A dropped, added, or misrenamed
/// table, index, or trigger fails here by name, independently of the digest.
const EXPECTED_OBJECTS: &[(&str, &str)] = &[
    ("index", "idx_authority_project"),
    ("index", "idx_authority_route_bindings_authority"),
    ("index", "idx_changefeed_domain_seq"),
    ("index", "idx_channel1_appends_session"),
    ("index", "idx_chunk_transcripts_session_range"),
    ("index", "idx_claim_intents_unresolved"),
    ("index", "idx_claim_mirror_claims_project"),
    ("index", "idx_compartment_events_session"),
    ("index", "idx_compartments_session_end_message"),
    ("index", "idx_dreamer_attempts_project_dispatched"),
    ("index", "idx_facade_mutation_ledger_scope_newest"),
    ("index", "idx_field_scans_batch"),
    ("index", "idx_historian_side_channel_outbox_due"),
    ("index", "idx_historian_side_channel_outbox_order"),
    ("index", "idx_note_deliveries_retry"),
    ("index", "idx_note_eval_claims_active_note"),
    ("index", "idx_note_eval_claims_active_slot"),
    ("index", "idx_notes_due"),
    ("index", "idx_notes_project_status_updated"),
    ("index", "idx_notes_scope_status"),
    ("index", "idx_pending_agent_drops_command"),
    ("index", "idx_pending_agent_drops_session"),
    ("index", "idx_primer_candidates_project"),
    ("index", "idx_primer_candidates_session"),
    ("index", "idx_recomp_commands_session_created"),
    ("index", "idx_reduce_command_ledger_session_newest"),
    ("index", "idx_scan_owner_copies_domain_owner"),
    ("index", "idx_scan_owner_copies_scan"),
    ("index", "idx_temporal_marks_session_created"),
    ("index", "idx_transform_session_roots_observed"),
    ("index", "idx_user_hints_session_created"),
    ("index", "idx_user_memories_status"),
    ("index", "idx_user_memory_candidates_session"),
    ("index", "idx_workspace_member_unique"),
    ("index", "idx_wrapup_commands_session_created"),
    ("table", "authority"),
    ("table", "authority_route_bindings"),
    ("table", "authority_seed_rows"),
    ("table", "cache_state"),
    ("table", "changefeed"),
    ("table", "channel1_appends"),
    ("table", "chunk_transcripts"),
    ("table", "claim_intent_controls"),
    ("table", "claim_intents"),
    ("table", "claim_mirror_claims"),
    ("table", "claim_mirror_projects"),
    ("table", "claim_mirror_receipts"),
    ("table", "claim_mirror_state"),
    ("table", "compartment_events"),
    ("table", "compartments"),
    ("table", "dreamer_attempts"),
    ("table", "dreamer_receipts"),
    ("table", "facade_mutation_ledger"),
    ("table", "fence"),
    ("table", "field_scans"),
    ("table", "format_marker"),
    ("table", "historian_side_channel_outbox"),
    ("table", "note_deliveries"),
    ("table", "note_eval_acquisitions"),
    ("table", "note_eval_claims"),
    ("table", "notes"),
    ("table", "overlay_frontiers"),
    ("table", "pass_trace"),
    ("table", "pending_agent_drops"),
    ("table", "primer_candidates"),
    ("table", "project_mural_artifacts"),
    ("table", "recomp_commands"),
    ("table", "reduce_command_ledger"),
    ("table", "scan_batches"),
    ("table", "scan_detections"),
    ("table", "scan_domain_owners"),
    ("table", "scan_owner_copies"),
    ("table", "scan_owner_scopes"),
    ("table", "tag_cache_generations"),
    ("table", "tags"),
    ("table", "temporal_marks"),
    ("table", "transform_session_roots"),
    ("table", "user_hints"),
    ("table", "user_memories"),
    ("table", "user_memory_candidates"),
    ("table", "workspace_members"),
    ("table", "workspaces"),
    ("table", "wrapup_commands"),
    ("trigger", "notes_facade_authority_delete"),
    ("trigger", "notes_facade_authority_insert"),
    ("trigger", "notes_facade_authority_update"),
    ("trigger", "notes_feed_delete"),
    ("trigger", "notes_feed_insert"),
    ("trigger", "notes_feed_update"),
    ("trigger", "notes_ownership_delete"),
    ("trigger", "notes_ownership_insert"),
    ("trigger", "notes_ownership_update"),
    ("trigger", "tags_cache_generation_delete"),
    ("trigger", "tags_cache_generation_insert"),
    ("trigger", "tags_cache_generation_update"),
];

/// The objects `storage::open_sqlite` installs ahead of the consumer's baseline,
/// followed by this crate's baseline; together they are the whole schema.
fn expected_inventory() -> Vec<storage::SchemaObject> {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(STORE_BASELINE).unwrap();
    conn.execute_batch(include_str!("../baseline.sql")).unwrap();
    schema_inventory(&conn).unwrap()
}

fn inspect(dir: &std::path::Path) -> Connection {
    Connection::open_with_flags(
        dir.join("memory.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

fn notes_columns(conn: &Connection) -> BTreeSet<String> {
    conn.prepare("SELECT name FROM pragma_table_info('notes')")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn fresh_open_creates_memory_sqlite_with_the_eidnara_identity_and_the_whole_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open_for_test(dir.path(), "eidnara-test");
    drop(store);

    let conn = inspect(dir.path());
    let application_id: u32 = conn
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .unwrap();
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    // ASCII `EIDN`, the value every Eidnara store family stamps.
    assert_eq!(application_id, 0x4549_444E);
    assert_eq!(user_version, 1);

    let expected = expected_inventory();
    assert_eq!(schema_inventory(&conn).unwrap(), expected);
    let own: Vec<(String, String)> = expected
        .iter()
        .filter(|o| !o.name.starts_with("sqlite_"))
        .map(|o| (o.kind.clone(), o.name.clone()))
        .collect();
    let pinned: Vec<(String, String)> = EXPECTED_OBJECTS
        .iter()
        .map(|(kind, name)| ((*kind).to_string(), (*name).to_string()))
        .collect();
    assert_eq!(own, pinned);

    let store_only = Connection::open_in_memory().unwrap();
    store_only.execute_batch(STORE_BASELINE).unwrap();
    let store_tables: BTreeSet<String> = schema_inventory(&store_only)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == "table" && !o.name.starts_with("sqlite_"))
        .map(|o| o.name)
        .collect();
    let infrastructure: BTreeSet<String> = INFRASTRUCTURE_TABLES
        .iter()
        .map(|t| (*t).to_string())
        .collect();
    assert_eq!(store_tables, infrastructure);
    for table in INFRASTRUCTURE_TABLES {
        assert!(
            EXPECTED_OBJECTS.contains(&("table", table)),
            "infrastructure table {table} is not pinned"
        );
    }
}

/// `notes` triggers require scalar functions that only `MemoryStore::open` registers,
/// so a raw connection cannot mutate `notes`. commentlint: allow(JUDGE)
#[test]
fn a_raw_connection_cannot_mutate_notes_because_the_trigger_functions_are_unregistered() {
    const TRIGGER_FUNCTIONS: [&str; 3] = [
        "note_caller_project",
        "facade_authority_domain",
        "facade_authority_route",
    ];
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open_for_test(dir.path(), "eidnara-test");
    let seeded = store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "seeded through the store",
            surface_condition: None,
            anchor_block_id: None,
            now_ms: 1,
        })
        .unwrap();
    drop(store);

    let conn = Connection::open(dir.path().join("memory.sqlite")).unwrap();
    // Negative control: `cache_state` has no triggers, so this raw write succeeds and
    // the `notes` failures are the unregistered functions, not a read-only file or
    // a held lease. commentlint: allow(JUDGE)
    conn.execute(
        "INSERT INTO cache_state (session_id, row_version, core_state, meta)
         VALUES ('raw-session', 1, '{}', '{}')",
        [],
    )
    .unwrap();

    // SQLite resolves trigger functions at statement preparation and reports only the
    // first unresolved function, so the reported name depends on trigger creation
    // order. commentlint: allow(JUDGE)
    for sql in [
        "INSERT INTO notes (project_path, content) VALUES ('project', 'raw')",
        "UPDATE notes SET content = 'x' WHERE id = ?1",
        "DELETE FROM notes WHERE id = ?1",
    ] {
        let error = if sql.starts_with("INSERT") {
            conn.execute(sql, [])
        } else {
            conn.execute(sql, [seeded.id])
        }
        .unwrap_err()
        .to_string();
        assert!(error.contains("no such function: "), "{sql}: {error}");
        assert!(
            TRIGGER_FUNCTIONS.iter().any(|f| error.contains(f)),
            "{sql}: {error}"
        );
    }
    let (count, content): (i64, String) = conn
        .query_row(
            "SELECT COUNT(*), MAX(content) FROM notes WHERE id = ?1",
            [seeded.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((count, content.as_str()), (1, "seeded through the store"));
}

#[test]
fn notes_changefeed_triggers_snapshot_and_watch_every_column() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open_for_test(dir.path(), "eidnara-test");
    let conn = Connection::open(dir.path().join("memory.sqlite")).unwrap();
    let columns = notes_columns(&conn);
    assert!(columns.len() >= 40, "{columns:?}");

    assert!(
        store
            .pull_changefeed("notes", 0, 100)
            .unwrap()
            .rows
            .is_empty()
    );
    let note = store
        .insert_note(NoteInput {
            project_path: "project",
            route_project_root: None,
            session_id: "session",
            content: "first",
            surface_condition: None,
            anchor_block_id: None,
            now_ms: 1,
        })
        .unwrap();
    let outcome = store
        .update_note_cas(
            "project",
            note.id,
            "active",
            note.status_version,
            Some("second"),
            None,
            None,
            2,
        )
        .unwrap();
    assert!(matches!(outcome, NoteCasOutcome::Applied(_)));
    store.delete_session("session", "project").unwrap();
    assert!(
        store
            .read_notes("project", "session", 10, 0)
            .unwrap()
            .is_empty()
    );

    let page = store.pull_changefeed("notes", 0, 100).unwrap();
    let ops: Vec<&str> = page.rows.iter().map(|row| row.op.as_str()).collect();
    // One feed row per API mutation. commentlint: allow(JUDGE)
    assert_eq!(ops, ["insert", "update", "tombstone"]);
    for row in &page.rows {
        assert_eq!(row.module_row_id, note.id);
        let keys: BTreeSet<String> = row
            .full_row_snapshot
            .as_object()
            .unwrap_or_else(|| panic!("{} snapshot is not an object", row.op))
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            keys,
            columns,
            "{} snapshot: missing {:?}, extra {:?}",
            row.op,
            columns.difference(&keys).collect::<Vec<_>>(),
            keys.difference(&columns).collect::<Vec<_>>()
        );
    }

    let trigger_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'notes_feed_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let when_clause = trigger_sql
        .split_once("WHEN")
        .and_then(|(_, rest)| rest.split_once("BEGIN"))
        .map(|(when, _)| when)
        .unwrap_or_else(|| panic!("no WHEN clause in {trigger_sql}"));
    // Whole-token matching: `NEW.status` is not satisfied by `NEW.status_version`.
    let watched: BTreeSet<&str> = when_clause
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .filter_map(|token| {
            token
                .strip_prefix("NEW.")
                .or_else(|| token.strip_prefix("OLD."))
        })
        .collect();
    let unwatched: Vec<&String> = columns
        .iter()
        .filter(|col| col.as_str() != "id" && !watched.contains(col.as_str()))
        .collect();
    assert!(
        unwatched.is_empty(),
        "notes_feed_update WHEN clause does not compare {unwatched:?}"
    );
}

/// A reopen must accept the file without re-laying the baseline: the schema is
/// unchanged and a row written before the reopen is still there after it.
#[test]
fn reopening_the_same_file_keeps_the_schema_and_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open_for_test(dir.path(), "eidnara-test");
    store
        .bind_authority_route("00112233445566778899aabbccddeeff", "git:proj", "/repo")
        .unwrap();
    drop(store);
    let before = schema_inventory(&inspect(dir.path())).unwrap();

    drop(MemoryStore::open_for_test(dir.path(), "eidnara-test"));
    let conn = inspect(dir.path());
    assert_eq!(schema_inventory(&conn).unwrap(), before);
    let bound: String = conn
        .query_row(
            "SELECT project FROM authority_route_bindings WHERE route_project_root = '/repo'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bound, "git:proj");
}

/// A file laid down under any other baseline text, even one differing by a
/// comment, is refused without mutation: the baseline bytes are the identity.
#[test]
fn a_file_with_a_different_baseline_is_refused_as_a_baseline_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let descriptor = MemoryStore::test_descriptor(dir.path(), "eidnara-test");
    let other_baseline = format!("{}\n-- one more comment\n", include_str!("../baseline.sql"));
    drop(storage::open_sqlite(&descriptor, &other_baseline).unwrap());
    let before = std::fs::read(dir.path().join("memory.sqlite")).unwrap();

    let Err(error) = MemoryStore::open(&descriptor) else {
        panic!("a file laid down under another baseline must not open");
    };
    // The comment-only difference leaves every schema object in place, so the digest
    // arm is the one that fires.
    assert!(
        matches!(
            &error,
            MemoryStoreError::Store(StoreError::Baseline(message))
                if message.contains("does not match the baseline digest")
        ),
        "{error}"
    );
    assert_eq!(
        std::fs::read(dir.path().join("memory.sqlite")).unwrap(),
        before
    );
}
