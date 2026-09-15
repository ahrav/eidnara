//! Expected column texts and effective terms are written by hand, never produced by the analyzer.

use std::num::NonZeroUsize;
use std::path::Path;

use kernel::Sensitivity;
use kernel::source_identity::Occurrence;
use retrieval::batch::{
    BatchBounds, BatchFault, BatchStatus, Invalidation, MutationIdentity, ProjectionBatch,
    VectorGeneration, apply_batch, apply_batch_with_fault_for_test, batch_status,
    register_generation,
};
use retrieval::coverage::verify_pages;
use retrieval::lexical::{
    AnalysisIdentity, LexicalBounds, analyze, compile, probe_engine, rowid, rowids, verify_rows,
};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity, Tombstone,
    TombstoneReason, install_identity,
};
use rusqlite::{Connection, params};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const KERNEL: &str = "kernel-1";
const HOLD: &str = "hold-1";
const GENERATION: &str = "gen-1";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn open(dir: &Path) -> SqliteStore {
    let store = open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir
                    .join("search")
                    .join("search.sqlite")
                    .to_string_lossy()
                    .into_owned(),
            },
        },
        retrieval::BASELINE,
    )
    .unwrap();
    store
        .with_conn_fenced(|conn| {
            install_identity(conn, &identity(), 1).unwrap();
            assert!(
                register_generation(
                    conn,
                    &VectorGeneration {
                        generation_id: GENERATION.to_string(),
                        embedding_model: "model-a".to_string(),
                        tokenizer_fingerprint: "fp-a".to_string(),
                        vector_dimension: 8,
                        generation_epoch: 1,
                    },
                    1,
                )
                .unwrap()
            );
            Ok(())
        })
        .unwrap();
    store
}

fn identity() -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: KERNEL.to_string(),
        projection_policy_version: "source-policy.v1".to_string(),
        identity_contract_version: "search-projection-identity-v3".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        analysis_identity: AnalysisIdentity::current().as_str().to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    }
}

fn mutation(snapshot: i64, through: i64) -> MutationIdentity {
    MutationIdentity {
        kernel_incarnation_id: KERNEL.to_string(),
        hold_id: HOLD.to_string(),
        snapshot_commit_seq: snapshot,
        through_commit_seq: through,
    }
}

/// One source the test owns; the class decides which identity fields the encoder needs.
struct Source {
    class: &'static str,
    key: &'static str,
    text: &'static str,
    created: i64,
}

impl Source {
    fn identity(&self) -> Vec<(String, String)> {
        match self.class {
            "messages" => vec![
                ("project_id".into(), "proj-a".into()),
                ("harness".into(), "opencode".into()),
                ("session_id".into(), "sess-01".into()),
                ("message_id".into(), format!("msg-{}", self.key)),
                ("block_index".into(), "0".into()),
            ],
            "canonical_claims" => vec![("object_id".into(), format!("obj-{}", self.key))],
            "promoted_memory" => vec![(
                "decision_object_id".into(),
                format!("decision-{}", self.key),
            )],
            "git_commits" => vec![
                ("repository_id".into(), "repo-a".into()),
                ("object_format".into(), "sha1".into()),
                ("oid".into(), format!("{:0>40}", self.key)),
            ],
            "raw_tool_spans" => vec![
                ("project_id".into(), "proj-a".into()),
                ("harness".into(), "pi".into()),
                ("session_id".into(), "sess-01".into()),
                ("parent_message_id".into(), "msg-2".into()),
                ("tool_call_id".into(), format!("call-{}", self.key)),
                ("result_revision".into(), "1".into()),
                ("block_index".into(), "0".into()),
            ],
            other => panic!("{other}"),
        }
    }

    fn representation(&self) -> &'static str {
        match self.class {
            "messages" => "text",
            "canonical_claims" => "decision_summary",
            "promoted_memory" => "summary",
            "git_commits" => "commit_message",
            "raw_tool_spans" => "tool_output",
            other => panic!("{other}"),
        }
    }
}

struct Arena {
    identities: Vec<Vec<(String, String)>>,
}

fn borrow(arena: &Arena) -> Vec<Vec<(&str, &str)>> {
    arena
        .identities
        .iter()
        .map(|fields| {
            fields
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect()
        })
        .collect()
}

fn records<'a>(
    sources: &'a [Source],
    borrowed: &'a [Vec<(&'a str, &'a str)>],
) -> Vec<OccurrenceRecord<'a>> {
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| OccurrenceRecord {
            occurrence: Occurrence {
                class: source.class,
                identity: &borrowed[index],
                revision: "1",
                representation: source.representation(),
                span: None,
            },
            payload: Payload::Whole(source.text),
            domain_id: "domain",
            sensitivity: Sensitivity::Normal,
            source_object_id: source.key,
            source_evidence_id: source.key,
            source_artifact_digest: DIGEST,
            created_commit_seq: source.created,
        })
        .collect()
}

fn occurrence_id(record: &OccurrenceRecord<'_>) -> String {
    kernel::source_identity::encode_preserving_span(&record.occurrence)
        .unwrap()
        .occurrence_id
}

fn apply(
    store: &SqliteStore,
    batch: &ProjectionBatch<'_>,
    fault: Option<BatchFault>,
) -> Result<(), ProjectionError> {
    let mut outcome = None;
    let _ = store.with_conn_fenced(|conn| {
        let result = match fault {
            Some(fault) => apply_batch_with_fault_for_test(conn, batch, bounds(), 10, fault),
            None => apply_batch(conn, batch, bounds(), 10),
        };
        let failed = result.is_err();
        outcome = Some(result.map(|_| ()));
        if failed {
            Err(rusqlite::Error::QueryReturnedNoRows)
        } else {
            Ok(())
        }
    });
    outcome.unwrap()
}

type LexicalRow = (i64, String, String, String);

fn lexical_rows(conn: &GuardedConn<'_>) -> Vec<LexicalRow> {
    conn.prepare("SELECT rowid, original, parts, occurrence_id FROM lexical ORDER BY rowid")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn matches(conn: &GuardedConn<'_>, request: &str) -> Vec<Vec<String>> {
    let analysis = analyze(
        request,
        LexicalBounds {
            max_input_bytes: NonZeroUsize::new(1024).unwrap(),
            max_atoms: NonZeroUsize::new(64).unwrap(),
        },
    )
    .unwrap();
    compile(&analysis)
        .iter()
        .map(|probe| {
            conn.prepare(
                "SELECT occurrence_id FROM lexical WHERE lexical MATCH ?1 ORDER BY occurrence_id",
            )
            .unwrap()
            .query_map([probe], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        })
        .collect()
}

fn with_conn<T>(store: &SqliteStore, f: impl FnOnce(&GuardedConn<'_>) -> T) -> T {
    let mut out = None;
    store
        .with_conn(|conn| {
            out = Some(f(conn));
            Ok(())
        })
        .unwrap();
    out.unwrap()
}

#[test]
fn every_retained_class_gets_a_lexical_row_with_the_hand_written_columns() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "restart the HTTPServer",
            created: 1,
        },
        Source {
            class: "canonical_claims",
            key: "c1",
            text: "snake_case wins",
            created: 1,
        },
        Source {
            class: "promoted_memory",
            key: "p1",
            text: "prefer x86_64 builds",
            created: 1,
        },
        Source {
            class: "git_commits",
            key: "abc123",
            text: "Fix E0308 in parser",
            created: 1,
        },
        Source {
            class: "raw_tool_spans",
            key: "t1",
            text: "ERR_CONN_REFUSED at 10.0.0.1:8080\0trailing",
            created: 1,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let ids: Vec<String> = records.iter().map(occurrence_id).collect();
    let batch = ProjectionBatch {
        identity: mutation(0, 1),
        records: records.clone(),
        invalidations: Vec::new(),
        generation_id: Some(GENERATION),
    };
    apply(&store, &batch, None).unwrap();
    let expected: Vec<LexicalRow> = {
        let columns = [
            ("restart the HTTPServer", "HTTP Server"),
            ("snake_case wins", "snake case"),
            ("prefer x86_64 builds", "x 86 64"),
            ("Fix E0308 in parser", "E 0308"),
            (
                "ERR_CONN_REFUSED at 10 0 0 1 8080 trailing",
                "ERR CONN REFUSED",
            ),
        ];
        let mut rows: Vec<LexicalRow> = ids
            .iter()
            .zip(columns)
            .map(|(id, (original, parts))| {
                (
                    rowid(id).unwrap(),
                    original.to_string(),
                    parts.to_string(),
                    id.clone(),
                )
            })
            .collect();
        rows.sort();
        rows
    };
    with_conn(&store, |conn| {
        assert_eq!(lexical_rows(conn), expected);
        assert_eq!(
            matches(conn, "REFUSED"),
            [vec![ids[4].clone()]],
            "a raw tool span is indexed without any embedding"
        );
        assert_eq!(matches(conn, "HTTP"), [vec![ids[0].clone()]]);
        assert_eq!(
            matches(conn, "trailing"),
            [vec![ids[4].clone()]],
            "NUL is a separator at index time"
        );
        assert_eq!(verify_rows(conn), Ok(()));
        assert_eq!(verify_pages(conn), Ok(()));
    });
}

#[test]
fn equal_byte_occurrences_stay_distinct_and_a_tombstone_removes_only_its_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "shared bytes here",
            created: 1,
        },
        Source {
            class: "messages",
            key: "m2",
            text: "shared bytes here",
            created: 1,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let ids: Vec<String> = records.iter().map(occurrence_id).collect();
    assert_ne!(ids[0], ids[1]);
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 1),
            records: records.clone(),
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    let mut both = ids.clone();
    both.sort();
    with_conn(&store, |conn| {
        assert_eq!(lexical_rows(conn).len(), 2, "equal bytes are two rows");
        assert_eq!(matches(conn, "shared"), [both.clone()]);
    });
    let invalidations = [Invalidation {
        occurrence_id: ids[0].clone(),
        tombstone: Tombstone {
            invalidated_commit_seq: 2,
            reason: TombstoneReason::Retired,
        },
    }];
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 2),
            records: Vec::new(),
            invalidations: invalidations.to_vec(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    with_conn(&store, |conn| {
        let rows = lexical_rows(conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].3, ids[1], "the sibling keeps its row");
        assert_eq!(matches(conn, "shared"), [vec![ids[1].clone()]]);
        assert_eq!(verify_rows(conn), Ok(()));
    });
}

#[test]
fn replay_neither_resurrects_a_tombstoned_row_nor_doubles_terms() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "alpha beta",
            created: 1,
        },
        Source {
            class: "messages",
            key: "m2",
            text: "alpha gamma alpha snake_case",
            created: 1,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let ids: Vec<String> = records.iter().map(occurrence_id).collect();
    let first = ProjectionBatch {
        identity: mutation(0, 1),
        records: records.clone(),
        invalidations: Vec::new(),
        generation_id: Some(GENERATION),
    };
    apply(&store, &first, None).unwrap();
    let invalidations = [Invalidation {
        occurrence_id: ids[0].clone(),
        tombstone: Tombstone {
            invalidated_commit_seq: 2,
            reason: TombstoneReason::Superseded,
        },
    }];
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 2),
            records: Vec::new(),
            invalidations: invalidations.to_vec(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    let before = (with_conn(&store, lexical_rows), term_instances(dir.path()));
    apply(&store, &first, None).unwrap();
    let after = (with_conn(&store, lexical_rows), term_instances(dir.path()));
    assert_eq!(
        after, before,
        "an older prefix leaves every lexical row and term count unchanged"
    );
    assert_eq!(after.0.len(), 1);
    assert_eq!(after.0[0].3, ids[1]);
    let row = rowid(&ids[1]).unwrap();
    let instance =
        |term: &str, col: &str, offset: i64| (term.to_string(), row, col.to_string(), offset);
    assert_eq!(
        after.1,
        [
            instance("alpha", "original", 0),
            instance("gamma", "original", 1),
            instance("alpha", "original", 2),
            instance("snake_case", "original", 3),
            instance("snake", "parts", 0),
            instance("case", "parts", 1),
        ],
        "the live row's terms appear once per occurrence in the text, with the repeated `alpha` twice, and the tombstoned row contributes none"
    );
}

#[test]
fn a_revision_with_an_equal_byte_sibling_replaces_only_its_own_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "same words",
            created: 1,
        },
        Source {
            class: "messages",
            key: "m2",
            text: "same words",
            created: 1,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let ids: Vec<String> = records.iter().map(occurrence_id).collect();
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 1),
            records: records.clone(),
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    let mut revised = records[0].clone();
    revised.occurrence.revision = "2";
    revised.created_commit_seq = 2;
    let revised_id = occurrence_id(&revised);
    assert_ne!(revised_id, ids[0]);
    let supersede = vec![Invalidation {
        occurrence_id: ids[0].clone(),
        tombstone: Tombstone {
            invalidated_commit_seq: 2,
            reason: TombstoneReason::Superseded,
        },
    }];
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 2),
            records: vec![revised],
            invalidations: supersede,
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    let mut expected = vec![revised_id.clone(), ids[1].clone()];
    expected.sort();
    with_conn(&store, |conn| {
        let rows: Vec<String> = lexical_rows(conn).into_iter().map(|row| row.3).collect();
        let mut rows = rows;
        rows.sort();
        assert_eq!(
            rows, expected,
            "revision 2 and the sibling; revision 1 is gone"
        );
        assert_eq!(matches(conn, "same"), [expected.clone()]);
        assert_eq!(verify_rows(conn), Ok(()));
    });
}

#[test]
fn batch_status_reads_lexical_presence_and_reapplying_restores_a_missing_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "keep me",
            created: 1,
        },
        Source {
            class: "messages",
            key: "m2",
            text: "drop me",
            created: 1,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let ids: Vec<String> = records.iter().map(occurrence_id).collect();
    let first = ProjectionBatch {
        identity: mutation(0, 1),
        records: records.clone(),
        invalidations: Vec::new(),
        generation_id: Some(GENERATION),
    };
    apply(&store, &first, None).unwrap();
    let path = dir.path().join("search").join("search.sqlite");
    let raw = Connection::open(&path).unwrap();
    raw.execute("DELETE FROM lexical WHERE occurrence_id=?1", [&ids[0]])
        .unwrap();
    drop(raw);
    with_conn(&store, |conn| {
        assert_eq!(
            batch_status(conn, &first),
            Ok(BatchStatus::NotApplied),
            "a live record without its row is not applied"
        );
    });
    apply(&store, &first, None).unwrap();
    with_conn(&store, |conn| {
        assert_eq!(
            batch_status(conn, &first),
            Ok(BatchStatus::Applied),
            "reapplying the same batch restores the row"
        );
        assert_eq!(verify_rows(conn), Ok(()));
    });
    let retire = ProjectionBatch {
        identity: mutation(0, 2),
        records: Vec::new(),
        invalidations: vec![Invalidation {
            occurrence_id: ids[1].clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 2,
                reason: TombstoneReason::Retired,
            },
        }],
        generation_id: Some(GENERATION),
    };
    apply(&store, &retire, None).unwrap();
    with_conn(&store, |conn| {
        assert_eq!(batch_status(conn, &retire), Ok(BatchStatus::Applied))
    });
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "INSERT INTO lexical(rowid, original, parts, occurrence_id) VALUES (?1, 'drop me', '', ?2)",
        params![rowid(&ids[1]).unwrap(), ids[1]],
    )
    .unwrap();
    drop(raw);
    with_conn(&store, |conn| {
        assert_eq!(
            batch_status(conn, &retire),
            Ok(BatchStatus::NotApplied),
            "a tombstoned record with a lingering row is not applied"
        );
        assert_eq!(verify_rows(conn), Err(ProjectionError::CorruptRow));
    });
    apply(&store, &retire, None).unwrap();
    with_conn(&store, |conn| {
        assert_eq!(
            batch_status(conn, &retire),
            Ok(BatchStatus::Applied),
            "reapplying the invalidation deletes the lingering row"
        );
        assert_eq!(verify_rows(conn), Ok(()));
    });
}

#[test]
fn the_payload_byte_bound_is_the_lexical_bound() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let exact: &'static str = Box::leak("a ".repeat(2048).trim_end().to_string().into_boxed_str());
    assert_eq!(exact.len(), 4095);
    let over: &'static str = Box::leak(format!("{exact}bb").into_boxed_str());
    let exact_source = [Source {
        class: "messages",
        key: "m1",
        text: exact,
        created: 1,
    }];
    let arena = Arena {
        identities: exact_source.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let exact_records = records(&exact_source, &borrowed);
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 1),
            records: exact_records,
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    with_conn(&store, |conn| {
        assert_eq!(lexical_rows(conn).len(), 1);
        assert_eq!(
            matches(conn, "a")[0].len(),
            1,
            "2048 one-byte atoms fit under a 4096-byte payload bound"
        );
    });
    let over_source = [Source {
        class: "messages",
        key: "m2",
        text: over,
        created: 2,
    }];
    let arena = Arena {
        identities: over_source.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let over_records = records(&over_source, &borrowed);
    assert!(matches!(
        apply(
            &store,
            &ProjectionBatch {
                identity: mutation(0, 2),
                records: over_records,
                invalidations: Vec::new(),
                generation_id: Some(GENERATION)
            },
            None
        ),
        Err(ProjectionError::OverBound { .. })
    ));
    with_conn(&store, |conn| {
        assert_eq!(
            lexical_rows(conn).len(),
            1,
            "the over-bound payload indexed nothing"
        )
    });
}

/// `(term, rowid, column, offset)` for every indexed term occurrence, from `fts5vocab` on a separate connection because the guarded connection permits no DDL.
fn term_instances(dir: &Path) -> Vec<(String, i64, String, i64)> {
    let raw = Connection::open(dir.join("search").join("search.sqlite")).unwrap();
    raw.execute_batch(
        "CREATE VIRTUAL TABLE temp.lexical_terms USING fts5vocab('main', 'lexical', 'instance')",
    )
    .unwrap();
    raw.prepare("SELECT term, doc, col, offset FROM temp.lexical_terms ORDER BY doc, col, offset")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn a_fault_after_lexical_rows_leaves_no_lexical_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [Source {
        class: "messages",
        key: "m1",
        text: "durable or nothing",
        created: 1,
    }];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let batch = ProjectionBatch {
        identity: mutation(0, 1),
        records: records.clone(),
        invalidations: Vec::new(),
        generation_id: Some(GENERATION),
    };
    assert!(matches!(
        apply(&store, &batch, Some(BatchFault::AfterLexical)),
        Err(ProjectionError::Sqlite(_))
    ));
    with_conn(&store, |conn| {
        assert!(lexical_rows(conn).is_empty());
        assert_eq!(matches(conn, "durable"), [Vec::<String>::new()]);
        assert_eq!(verify_rows(conn), Ok(()));
    });
    apply(&store, &batch, None).unwrap();
    with_conn(&store, |conn| assert_eq!(lexical_rows(conn).len(), 1));
}

#[test]
fn a_rebuild_from_one_snapshot_stores_the_rows_incremental_application_stored() {
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "first HTTPServer note",
            created: 1,
        },
        Source {
            class: "canonical_claims",
            key: "c1",
            text: "second snake_case note",
            created: 1,
        },
        Source {
            class: "raw_tool_spans",
            key: "t1",
            text: "third E0308 note",
            created: 2,
        },
        Source {
            class: "messages",
            key: "m4",
            text: "fourth note a note",
            created: 3,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let ids: Vec<String> = records.iter().map(occurrence_id).collect();
    let retire_first = [Invalidation {
        occurrence_id: ids[0].clone(),
        tombstone: Tombstone {
            invalidated_commit_seq: 2,
            reason: TombstoneReason::Retired,
        },
    }];

    let incremental_dir = tempfile::tempdir().unwrap();
    let incremental = open(incremental_dir.path());
    apply(
        &incremental,
        &ProjectionBatch {
            identity: mutation(0, 1),
            records: records[..2].to_vec(),
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    apply(
        &incremental,
        &ProjectionBatch {
            identity: mutation(0, 2),
            records: records[2..3].to_vec(),
            invalidations: retire_first.to_vec(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    apply(
        &incremental,
        &ProjectionBatch {
            identity: mutation(0, 3),
            records: records[3..].to_vec(),
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();

    let rebuilt_dir = tempfile::tempdir().unwrap();
    let rebuilt = open(rebuilt_dir.path());
    // A fenced export at commit 3 carries the live rows in its own order and no invalidation before its snapshot.
    let exported = [records[3].clone(), records[1].clone(), records[2].clone()];
    apply(
        &rebuilt,
        &ProjectionBatch {
            identity: mutation(3, 3),
            records: exported.to_vec(),
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();

    let observe = |store: &SqliteStore, dir: &Path| {
        (
            with_conn(store, lexical_rows),
            term_instances(dir),
            with_conn(store, |conn| {
                matches(conn, "note HTTP snake a E0308 second")
            }),
            with_conn(store, |conn| ranked(conn, "note")),
        )
    };
    let (rows, terms, ordered, by_rank) = observe(&incremental, incremental_dir.path());
    assert_eq!(
        observe(&rebuilt, rebuilt_dir.path()),
        (
            rows.clone(),
            terms.clone(),
            ordered.clone(),
            by_rank.clone()
        )
    );
    assert_eq!(
        by_rank[0], ids[3],
        "the row that says `note` twice ranks first in both constructions"
    );
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|row| row.3 != ids[0]),
        "the retired row is absent from both"
    );
    assert_eq!(
        ordered[1],
        Vec::<String>::new(),
        "HTTP lived only in the retired row"
    );
    assert_eq!(ordered[2], [ids[1].clone()]);
    assert_eq!(ordered[4], [ids[2].clone()]);
    assert_eq!(
        terms
            .iter()
            .filter(|(term, _, _, _)| term == "note")
            .count(),
        4,
        "three live rows say `note`, one of them twice"
    );
}

/// Occurrence ids by ascending raw FTS rank, then occurrence id.
fn ranked(conn: &GuardedConn<'_>, request: &str) -> Vec<String> {
    let probe = compile(
        &analyze(
            request,
            LexicalBounds {
                max_input_bytes: NonZeroUsize::new(64).unwrap(),
                max_atoms: NonZeroUsize::new(1).unwrap(),
            },
        )
        .unwrap(),
    )
    .remove(0);
    conn.prepare(
        "SELECT occurrence_id FROM lexical WHERE lexical MATCH ?1 ORDER BY rank, occurrence_id",
    )
    .unwrap()
    .query_map([probe], |row| row.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn verify_rows_refuses_a_missing_or_orphaned_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "one",
            created: 1,
        },
        Source {
            class: "messages",
            key: "m2",
            text: "two",
            created: 1,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 1),
            records: records.clone(),
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    let path = dir.path().join("search").join("search.sqlite");
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "DELETE FROM lexical WHERE rowid=(SELECT min(rowid) FROM lexical)",
        [],
    )
    .unwrap();
    drop(raw);
    with_conn(&store, |conn| {
        assert_eq!(
            verify_rows(conn),
            Err(ProjectionError::CorruptRow),
            "a live occurrence without a row"
        );
        assert_eq!(verify_pages(conn), Err(ProjectionError::CorruptRow));
    });
    // The surviving row stays; a ghost at one of its own derived rowids restores the counts, so only the liveness check can refuse it.
    let ghost = "f".repeat(64);
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "INSERT INTO lexical(rowid, original, parts, occurrence_id) VALUES (?1, 'ghost', '', ?2)",
        params![rowid(&ghost).unwrap(), ghost],
    )
    .unwrap();
    drop(raw);
    with_conn(&store, |conn| {
        assert_eq!(
            lexical_rows(conn).len(),
            2,
            "counts match the live occurrences"
        );
        assert_eq!(
            verify_rows(conn),
            Err(ProjectionError::CorruptRow),
            "a row naming no live occurrence"
        );
    });
}

#[test]
fn integrity_check_detects_an_injected_inverted_index_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [Source {
        class: "messages",
        key: "m1",
        text: "indexed content",
        created: 1,
    }];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(0, 1),
            records: records.clone(),
            invalidations: Vec::new(),
            generation_id: Some(GENERATION),
        },
        None,
    )
    .unwrap();
    with_conn(&store, |conn| assert_eq!(verify_pages(conn), Ok(())));
    let raw = Connection::open(dir.path().join("search").join("search.sqlite")).unwrap();
    assert_eq!(raw.execute("UPDATE lexical_content SET c0='other words' WHERE id=(SELECT min(id) FROM lexical_content)", []).unwrap(), 1);
    drop(raw);
    with_conn(&store, |conn| {
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert!(
            integrity.to_ascii_lowercase().contains("fts5"),
            "{integrity}"
        );
        assert_eq!(verify_pages(conn), Err(ProjectionError::CorruptRow));
    });
}

#[test]
fn probe_engine_reports_the_linked_engine_and_classifies_a_missing_module() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    with_conn(&store, |conn| {
        let engine = probe_engine(conn).unwrap();
        assert_eq!(engine.sqlite_version, rusqlite::version());
        assert!(engine.sqlite_source_id.len() > 20);
        let options = conn
            .prepare("PRAGMA compile_options")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        eprintln!("SQLite review engine: {engine:?}; compile_options={options:?}");
    });
    let skewed = Connection::open_in_memory().unwrap();
    skewed.execute_batch(retrieval::BASELINE).unwrap();
    skewed
        .create_scalar_function(
            "sqlite_source_id",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok("another-build".to_string()),
        )
        .unwrap();
    assert!(matches!(
        probe_engine(&skewed),
        Err(ProjectionError::Unsupported { .. })
    ));
    // A file whose lexical table names a module this build does not link is what an engine without FTS5 sees.
    let raw = Connection::open_in_memory().unwrap();
    raw.execute_batch(
        "PRAGMA writable_schema=1;
         INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)
         VALUES ('table','lexical','lexical',0,'CREATE VIRTUAL TABLE lexical USING nosuchmodule(original, parts)');",
    )
    .unwrap();
    drop(raw);
    let raw = Connection::open_in_memory().unwrap();
    assert!(
        matches!(probe_engine(&raw), Err(ProjectionError::Sqlite(_))),
        "a database without the table is an ordinary engine error"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("module.sqlite");
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch(
        "PRAGMA writable_schema=1;
         INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)
         VALUES ('table','lexical','lexical',0,'CREATE VIRTUAL TABLE lexical USING nosuchmodule(original, parts)');",
    )
    .unwrap();
    drop(raw);
    let raw = Connection::open(&path).unwrap();
    assert!(
        matches!(probe_engine(&raw), Err(ProjectionError::Unsupported { .. })),
        "{:?}",
        probe_engine(&raw)
    );
}

#[test]
fn rowids_are_the_four_masked_words_of_the_identifier() {
    let id = "0000000000000000ff00000000000000ffffffffffffffff0123456789abcdef";
    assert_eq!(
        rowids(id),
        Some([0, 0x7f00_0000_0000_0000, i64::MAX, 0x0123_4567_89ab_cdef])
    );
    assert_eq!(rowid(id), Some(0));
    assert_eq!(
        rowids("0123456789abcdef"),
        None,
        "sixteen digits are one word"
    );
    assert_eq!(rowids(&"g".repeat(64)), None);
    assert_eq!(rowids(&"0".repeat(65)), None);
    assert_eq!(rowid("abc"), None);
}

fn squat(dir: &Path, id: &str, slots: &[usize]) {
    let raw = Connection::open(dir.join("search").join("search.sqlite")).unwrap();
    for slot in slots {
        raw.execute(
            "INSERT INTO lexical(rowid, original, parts, occurrence_id) VALUES (?1, 'squatter', '', ?2)",
            params![rowids(id).unwrap()[*slot], format!("squatter-{slot}")],
        )
        .unwrap();
    }
}

fn collision_batch<'a>(records: &[OccurrenceRecord<'a>]) -> ProjectionBatch<'a> {
    ProjectionBatch {
        identity: mutation(0, 1),
        records: records.to_vec(),
        invalidations: Vec::new(),
        generation_id: Some(GENERATION),
    }
}

#[test]
fn a_held_rowid_moves_the_row_to_the_next_word_without_refusing_the_batch() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [Source {
        class: "messages",
        key: "m1",
        text: "collides",
        created: 1,
    }];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let id = occurrence_id(&records[0]);
    squat(dir.path(), &id, &[0, 1]);
    let batch = collision_batch(&records);
    apply(&store, &batch, None).unwrap();
    with_conn(&store, |conn| {
        let mine: Vec<i64> = conn
            .prepare("SELECT rowid FROM lexical WHERE occurrence_id=?1")
            .unwrap()
            .query_map([&id], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(mine, [rowids(&id).unwrap()[2]], "the first free word");
        assert_eq!(batch_status(conn, &batch), Ok(BatchStatus::Applied));
        assert_eq!(matches(conn, "collides"), [[id.clone()]]);
    });
    // A replay finds the row where it sits and stores no second one.
    apply(&store, &batch, None).unwrap();
    let raw = Connection::open(dir.path().join("search").join("search.sqlite")).unwrap();
    raw.execute(
        "DELETE FROM lexical WHERE occurrence_id LIKE 'squatter-%'",
        [],
    )
    .unwrap();
    drop(raw);
    with_conn(&store, |conn| {
        assert_eq!(lexical_rows(conn).len(), 1);
        assert_eq!(
            verify_rows(conn),
            Ok(()),
            "a row at any of its words verifies"
        );
        assert_eq!(batch_status(conn, &batch), Ok(BatchStatus::Applied));
    });
}

#[test]
fn a_collision_on_every_word_is_refused_before_anything_persists() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [Source {
        class: "messages",
        key: "m1",
        text: "collides",
        created: 1,
    }];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let id = occurrence_id(&records[0]);
    squat(dir.path(), &id, &[0, 1, 2, 3]);
    let batch = collision_batch(&records);
    assert_eq!(
        apply(&store, &batch, None),
        Err(ProjectionError::LexicalRowidCollision {
            occurrence_id: id,
            holders: (0..4).map(|slot| format!("squatter-{slot}")).collect(),
        })
    );
    with_conn(&store, |conn| {
        let stored: i64 = conn
            .query_row("SELECT count(*) FROM occurrences", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, 0, "the refused batch persisted nothing");
    });
}

#[test]
fn tombstone_occurrence_itself_removes_the_lexical_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [
        Source {
            class: "messages",
            key: "m1",
            text: "retired soon",
            created: 1,
        },
        Source {
            class: "messages",
            key: "m2",
            text: "stays live",
            created: 1,
        },
    ];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let ids: Vec<String> = records.iter().map(occurrence_id).collect();
    apply(&store, &collision_batch(&records), None).unwrap();
    let stone = Tombstone {
        invalidated_commit_seq: 2,
        reason: TombstoneReason::Retired,
    };
    store
        .with_conn_fenced(|conn| {
            let first = retrieval::tombstone_occurrence(conn, &ids[0], stone, 5).unwrap();
            assert!(first.recorded);
            assert_eq!(first.lexical_rows_deleted, 1);
            let again = retrieval::tombstone_occurrence(conn, &ids[0], stone, 6).unwrap();
            assert!(!again.recorded);
            assert_eq!(again.lexical_rows_deleted, 0);
            Ok(())
        })
        .unwrap();
    with_conn(&store, |conn| {
        let remaining: Vec<String> = lexical_rows(conn).into_iter().map(|row| row.3).collect();
        assert_eq!(remaining, [ids[1].clone()]);
        assert_eq!(verify_rows(conn), Ok(()));
    });
}

#[test]
fn retiring_collision_holders_frees_replacement_slots_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let sources = [Source {
        class: "raw_tool_spans",
        key: "new",
        text: "replacement",
        created: 2,
    }];
    let arena = Arena {
        identities: sources.iter().map(Source::identity).collect(),
    };
    let borrowed = borrow(&arena);
    let records = records(&sources, &borrowed);
    let id = occurrence_id(&records[0]);
    let holders: Vec<String> = rowids(&id)
        .unwrap()
        .iter()
        .map(|word| format!("{word:016x}{}", "0".repeat(48)))
        .collect();
    store.with_conn_fenced(|conn| {
        // The digest seam forces collisions without moving a row outside its allowed slots.
        let mut holder = records[0].clone();
        holder.created_commit_seq = 1;
        holder.payload = Payload::Whole("holder");
        for holder_id in &holders {
            retrieval::persist_occurrences_with_digests_for_test(
                conn, &[holder.clone()], bounds().persist, 1,
                &|_, bytes| (holder_id.clone(), kernel::source_identity::identity_digest(bytes)),
            ).unwrap();
            conn.execute(
                "INSERT INTO lexical(rowid, original, parts, occurrence_id) VALUES (?1,'holder','',?2)",
                params![rowid(holder_id).unwrap(), holder_id],
            )?;
        }
        assert_eq!(verify_rows(conn), Ok(()));
        Ok(())
    }).unwrap();
    let batch = ProjectionBatch {
        identity: mutation(0, 2),
        records,
        invalidations: holders
            .into_iter()
            .map(|occurrence_id| Invalidation {
                occurrence_id,
                tombstone: Tombstone {
                    invalidated_commit_seq: 2,
                    reason: TombstoneReason::Retired,
                },
            })
            .collect(),
        generation_id: Some(GENERATION),
    };
    let before = with_conn(&store, lexical_rows);
    assert!(apply(&store, &batch, Some(BatchFault::AfterLexical)).is_err());
    assert_eq!(
        with_conn(&store, lexical_rows),
        before,
        "a later failure restores all holders"
    );
    apply(&store, &batch, None).unwrap();
    with_conn(&store, |conn| {
        assert_eq!(
            lexical_rows(conn),
            [(rowid(&id).unwrap(), "replacement".into(), "".into(), id)]
        );
        assert_eq!(batch_status(conn, &batch), Ok(BatchStatus::Applied));
        assert_eq!(verify_rows(conn), Ok(()));
    });
}

#[test]
fn a_row_storing_other_text_is_corrupt_on_replay_and_in_status() {
    for column in ["original", "parts"] {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        let sources = [Source {
            class: "messages",
            key: "m1",
            text: "genuine text",
            created: 1,
        }];
        let arena = Arena {
            identities: sources.iter().map(Source::identity).collect(),
        };
        let borrowed = borrow(&arena);
        let records = records(&sources, &borrowed);
        let batch = collision_batch(&records);
        apply(&store, &batch, None).unwrap();
        let raw = Connection::open(dir.path().join("search").join("search.sqlite")).unwrap();
        raw.execute(
            &format!("UPDATE lexical SET {column}='forged text' WHERE occurrence_id=?1"),
            [&occurrence_id(&records[0])],
        )
        .unwrap();
        assert_eq!(
            raw.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok",
            "a coherent update keeps the inverted index consistent"
        );
        drop(raw);
        with_conn(&store, |conn| {
            assert_eq!(batch_status(conn, &batch), Err(ProjectionError::CorruptRow));
            assert_eq!(verify_rows(conn), Err(ProjectionError::CorruptRow));
            assert_eq!(verify_pages(conn), Err(ProjectionError::CorruptRow));
        });
        assert_eq!(
            apply(&store, &batch, None),
            Err(ProjectionError::CorruptRow)
        );
    }
}
