//! Batches are applied against an independent ledger of what each batch
//! should leave durable, and every phase is failed to show the transaction
//! leaves the whole prior state or the whole committed state, never a mix.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::path::Path;

use kernel::Sensitivity;
use kernel::source_identity::{Occurrence, Span};
use retrieval::batch::{
    BatchBounds, BatchFault, BatchOutcome, BatchStatus, Invalidation, MutationIdentity,
    ProjectionBatch, VectorGeneration, apply_batch, apply_batch_with_fault_for_test, batch_status,
    register_generation,
};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity, Tombstone,
    TombstoneReason, install_identity, read_occurrence,
};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const HOLD: &str = "hold-1";
const GENERATION: &str = "gen-1";

fn open(dir: &Path) -> SqliteStore {
    open_sqlite(
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
    .unwrap()
}

fn identity() -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: "kernel-1".to_string(),
        projection_policy_version: "source-policy.v1".to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
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
        kernel_incarnation_id: "kernel-1".to_string(),
        hold_id: HOLD.to_string(),
        snapshot_commit_seq: snapshot,
        through_commit_seq: through,
    }
}

/// A source the test owns: class, key, revision, and text. Identity fields
/// are minted from the key.
#[derive(Debug, Clone)]
struct Source {
    class: &'static str,
    key: String,
    revision: i64,
    text: String,
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
            "raw_tool_spans" => "tool_output",
            other => panic!("{other}"),
        }
    }
}

/// Borrowed views a batch can point into.
struct Arena {
    identities: Vec<Vec<(String, String)>>,
    revisions: Vec<String>,
}

impl Arena {
    fn new(sources: &[Source]) -> Self {
        Self {
            identities: sources.iter().map(Source::identity).collect(),
            revisions: sources.iter().map(|s| s.revision.to_string()).collect(),
        }
    }
}

fn records<'a>(
    sources: &'a [Source],
    arena: &'a Arena,
    borrowed: &'a [Vec<(&'a str, &'a str)>],
) -> Vec<OccurrenceRecord<'a>> {
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| OccurrenceRecord {
            occurrence: Occurrence {
                class: source.class,
                identity: &borrowed[index],
                revision: &arena.revisions[index],
                representation: source.representation(),
                span: None,
            },
            payload: Payload::Whole(&source.text),
            domain_id: "domain",
            sensitivity: Sensitivity::Normal,
            source_object_id: &source.key,
            source_evidence_id: &source.key,
            source_artifact_digest: "0000000000000000000000000000000000000000000000000000000000000000",
            created_commit_seq: source.created,
        })
        .collect()
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

/// Everything durable the ledger predicts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Durable {
    occurrences: BTreeSet<String>,
    payloads: BTreeMap<String, i64>,
    tombstones: BTreeMap<String, (i64, String)>,
    checkpoint: Option<(i64, i64, String)>,
    pending: BTreeSet<String>,
    /// Job state keyed by `(occurrence_id, generation_id)`.
    jobs: BTreeMap<(String, String), String>,
}

fn durable(conn: &GuardedConn<'_>) -> Durable {
    fn rows<T>(
        conn: &GuardedConn<'_>,
        sql: &str,
        map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Vec<T> {
        let mut statement = conn.prepare(sql).unwrap();
        statement
            .query_map([], map)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
    let occurrences = rows(
        conn,
        "SELECT occurrence_id FROM occurrences ORDER BY 1",
        |r| r.get(0),
    )
    .into_iter()
    .collect();
    let tombstones = rows(
        conn,
        "SELECT occurrence_id,invalidated_commit_seq,reason FROM occurrence_tombstones ORDER BY 1",
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)?, r.get::<_, String>(2)?),
            ))
        },
    )
    .into_iter()
    .collect();
    let checkpoint = conn
        .query_row(
            "SELECT snapshot_commit_seq,checkpoint_commit_seq,hold_id FROM projection_checkpoint",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();
    let pending = rows(
        conn,
        "SELECT occurrence_id FROM embedding_jobs WHERE state='pending'",
        |row| row.get(0),
    )
    .into_iter()
    .collect();
    let jobs = rows(
        conn,
        "SELECT occurrence_id,generation_id,state FROM embedding_jobs ORDER BY 1,2",
        |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                r.get::<_, String>(2)?,
            ))
        },
    )
    .into_iter()
    .collect();
    let payloads = rows(
        conn,
        "SELECT payload_id,byte_length FROM payloads ORDER BY 1",
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )
    .into_iter()
    .collect();
    Durable {
        occurrences,
        payloads,
        tombstones,
        checkpoint,
        pending,
        jobs,
    }
}

fn apply(
    store: &SqliteStore,
    batch: &ProjectionBatch<'_>,
    now: i64,
) -> Result<BatchOutcome, ProjectionError> {
    let mut outcome = None;
    let _ = store.with_conn_fenced(|conn| {
        let result = apply_batch(conn, batch, bounds(), now);
        let failed = result.is_err();
        outcome = Some(result);
        if failed {
            Err(rusqlite::Error::QueryReturnedNoRows)
        } else {
            Ok(())
        }
    });
    outcome.unwrap()
}

fn setup(store: &SqliteStore) {
    store
        .with_conn_fenced(|conn| {
            install_identity(conn, &identity(), 1).unwrap();
            assert!(register_generation(conn, &generation(), 1).unwrap());
            assert!(!register_generation(conn, &generation(), 2).unwrap());
            Ok(())
        })
        .unwrap();
}

/// The identifier the kernel encoder gives a record; the ledger's own view of
/// identity, computed without the store.
fn occurrence_id(record: &OccurrenceRecord<'_>) -> String {
    kernel::source_identity::encode_preserving_span(&record.occurrence)
        .unwrap()
        .occurrence_id
}

#[test]
fn a_ledger_predicts_the_reopened_state_after_multi_ordinal_empty_and_control_batches() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let objects: Vec<String> = store
        .with_conn_unfenced(|conn| {
            let mut statement = conn.prepare(
                "SELECT type||':'||name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' AND name NOT IN ('fence','format_marker') ORDER BY 1",
            )?;
            statement.query_map([], |row| row.get(0))?.collect()
        })
        .unwrap();
    assert_eq!(
        objects,
        [
            "index:idx_embedding_jobs_dispatch",
            "index:idx_embedding_jobs_generation",
            "index:idx_embedding_jobs_open_order",
            "index:idx_occurrence_vectors_generation",
            "index:idx_occurrences_lineage",
            "index:idx_occurrences_payload",
            "index:idx_occurrences_source",
            "index:idx_retirement_receipts_generation",
            "index:idx_vector_generations_selected",
            "table:embedding_jobs",
            "table:embedding_recovery_authorizations",
            "table:occurrence_tombstones",
            "table:occurrence_vectors",
            "table:occurrences",
            "table:payloads",
            "table:projection_checkpoint",
            "table:projection_identity",
            "table:retirement_receipts",
            "table:vector_generations",
        ]
        .map(str::to_string),
        "the frozen inventory's objects and no others"
    );

    // Batch one: a multi-ordinal commit at S with three classes.
    let first = vec![
        Source {
            class: "messages",
            key: "m1".into(),
            revision: 1,
            text: "hello".into(),
            created: 3,
        },
        Source {
            class: "canonical_claims",
            key: "c1".into(),
            revision: 1,
            text: "claim".into(),
            created: 3,
        },
        Source {
            class: "raw_tool_spans",
            key: "t1".into(),
            revision: 1,
            text: "tool out".into(),
            created: 4,
        },
        Source {
            class: "messages",
            key: "m2".into(),
            revision: 1,
            text: "  spaced\n".into(),
            created: 5,
        },
    ];
    let arena = Arena::new(&first);
    let borrowed = borrow(&arena);
    let first_records = records(&first, &arena, &borrowed);
    let ids: Vec<String> = first_records.iter().map(|r| occurrence_id(r)).collect();
    let batch1 = ProjectionBatch {
        identity: mutation(5, 5),
        records: first_records.clone(),
        invalidations: vec![],
        generation_id: Some(GENERATION),
    };
    let outcome = apply(&store, &batch1, 10).unwrap();
    assert_eq!(
        outcome,
        BatchOutcome {
            rows_inserted: 4,
            rows_replayed: 0,
            tombstones_recorded: 0,
            pending_created: 3,
            pending_obsoleted: 0,
            checkpoint_commit_seq: 5,
        }
    );
    let job = |id: &String| (id.clone(), GENERATION.to_string());
    let mut ledger = Durable {
        occurrences: ids.iter().cloned().collect(),
        payloads: first
            .iter()
            .map(|source| {
                (
                    kernel::source_identity::payload_id(source.text.as_bytes()),
                    source.text.len() as i64,
                )
            })
            .collect(),
        tombstones: BTreeMap::new(),
        checkpoint: Some((5, 5, HOLD.to_string())),
        pending: [0, 1, 3].iter().map(|&i| ids[i].clone()).collect(),
        jobs: [0, 1, 3]
            .iter()
            .map(|&i| (job(&ids[i]), "pending".to_string()))
            .collect(),
    };
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), ledger);
            Ok(())
        })
        .unwrap();

    // Batch two: an empty commit window moves only the checkpoint.
    let empty = ProjectionBatch {
        identity: mutation(5, 7),
        records: vec![],
        invalidations: vec![],
        generation_id: Some(GENERATION),
    };
    assert_eq!(apply(&store, &empty, 11).unwrap().checkpoint_commit_seq, 7);
    ledger.checkpoint = Some((5, 7, HOLD.to_string()));
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), ledger);
            Ok(())
        })
        .unwrap();

    // Batch three: a control commit supersedes m1 with revision 2 and retires c1.
    let third = vec![Source {
        class: "messages",
        key: "m1".into(),
        revision: 2,
        text: "hello again".into(),
        created: 8,
    }];
    let arena3 = Arena::new(&third);
    let borrowed3 = borrow(&arena3);
    let third_records = records(&third, &arena3, &borrowed3);
    let m1r2 = occurrence_id(&third_records[0]);
    let batch3 = ProjectionBatch {
        identity: mutation(5, 9),
        records: third_records.clone(),
        invalidations: vec![
            Invalidation {
                occurrence_id: ids[0].clone(),
                tombstone: Tombstone {
                    invalidated_commit_seq: 8,
                    reason: TombstoneReason::Superseded,
                },
            },
            Invalidation {
                occurrence_id: ids[1].clone(),
                tombstone: Tombstone {
                    invalidated_commit_seq: 9,
                    reason: TombstoneReason::Retired,
                },
            },
        ],
        generation_id: Some(GENERATION),
    };
    let outcome = apply(&store, &batch3, 12).unwrap();
    assert_eq!(
        outcome,
        BatchOutcome {
            rows_inserted: 1,
            rows_replayed: 0,
            tombstones_recorded: 2,
            pending_created: 1,
            pending_obsoleted: 2,
            checkpoint_commit_seq: 9,
        }
    );
    ledger.occurrences.insert(m1r2.clone());
    ledger.payloads.insert(
        kernel::source_identity::payload_id(b"hello again"),
        "hello again".len() as i64,
    );
    ledger
        .tombstones
        .insert(ids[0].clone(), (8, "superseded".to_string()));
    ledger
        .tombstones
        .insert(ids[1].clone(), (9, "retired".to_string()));
    ledger.checkpoint = Some((5, 9, HOLD.to_string()));
    ledger.pending = [ids[3].clone(), m1r2.clone()].into_iter().collect();
    ledger.jobs.insert(job(&ids[0]), "obsolete".to_string());
    ledger.jobs.insert(job(&ids[1]), "obsolete".to_string());
    ledger.jobs.insert(job(&m1r2), "pending".to_string());
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), ledger);
            Ok(())
        })
        .unwrap();

    // Reopen: the ledger still holds, bytes included.
    drop(store);
    let store = open(dir.path());
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), ledger);
            let stored = read_occurrence(conn, &m1r2).unwrap().unwrap();
            assert_eq!(stored.bytes, b"hello again");
            let old = read_occurrence(conn, &ids[0]).unwrap().unwrap();
            assert_eq!(old.bytes, b"hello");
            assert_eq!(old.tombstone.map(|t| t.invalidated_commit_seq), Some(8));
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_fault_at_any_phase_leaves_the_whole_prior_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let base = vec![Source {
        class: "messages",
        key: "base".into(),
        revision: 1,
        text: "base".into(),
        created: 2,
    }];
    let arena = Arena::new(&base);
    let borrowed = borrow(&arena);
    let base_records = records(&base, &arena, &borrowed);
    let base_id = occurrence_id(&base_records[0]);
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(2, 2),
            records: base_records,
            invalidations: vec![],
            generation_id: Some(GENERATION),
        },
        1,
    )
    .unwrap();
    let prior = store.with_conn(|conn| Ok(durable(conn))).unwrap();

    let next = vec![
        Source {
            class: "messages",
            key: "n1".into(),
            revision: 1,
            text: "next".into(),
            created: 3,
        },
        Source {
            class: "canonical_claims",
            key: "n2".into(),
            revision: 1,
            text: "claim".into(),
            created: 4,
        },
    ];
    let arena2 = Arena::new(&next);
    let borrowed2 = borrow(&arena2);
    let batch = ProjectionBatch {
        identity: mutation(2, 4),
        records: records(&next, &arena2, &borrowed2),
        invalidations: vec![Invalidation {
            occurrence_id: base_id.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 3,
                reason: TombstoneReason::Retired,
            },
        }],
        generation_id: Some(GENERATION),
    };
    for at in [1, 2, 5] {
        let mut outside_window = batch.clone();
        outside_window.invalidations[0]
            .tombstone
            .invalidated_commit_seq = at;
        let result = store
            .with_conn_fenced(|conn| Ok(apply_batch(conn, &outside_window, bounds(), 5)))
            .unwrap();
        assert_eq!(
            result,
            Err(ProjectionError::MutationConflict),
            "invalidation at {at} is outside (2, 4]"
        );
        assert_eq!(
            store.with_conn(|conn| Ok(durable(conn))).unwrap(),
            prior,
            "a handled admission error commits no batch writes"
        );
    }
    let mut future_record = batch.clone();
    future_record.records[1].created_commit_seq = 5;
    let result = store
        .with_conn_fenced(|conn| Ok(apply_batch(conn, &future_record, bounds(), 5)))
        .unwrap();
    assert_eq!(
        result,
        Err(ProjectionError::MutationConflict),
        "a record created after the batch end is refused"
    );
    assert_eq!(
        store.with_conn(|conn| Ok(durable(conn))).unwrap(),
        prior,
        "a handled admission error commits no batch writes"
    );
    for fault in [
        BatchFault::AfterAdmission,
        BatchFault::AfterRows,
        BatchFault::AfterTombstones,
        BatchFault::AfterPending,
        BatchFault::AfterCheckpoint,
    ] {
        let result: Result<(), _> = store.with_conn_fenced(|conn| {
            match apply_batch_with_fault_for_test(conn, &batch, bounds(), 5, fault) {
                Err(ProjectionError::Sqlite(text)) if text.contains("injected") => {
                    Err(rusqlite::Error::QueryReturnedNoRows)
                }
                other => panic!("{fault:?}: {other:?}"),
            }
        });
        assert!(result.is_err(), "{fault:?}");
        // The transaction rolled back: the durable state is the prior state.
        store
            .with_conn(|conn| {
                assert_eq!(durable(conn), prior, "{fault:?}");
                assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::NotApplied);
                Ok(())
            })
            .unwrap();
    }
    // The same batch without a fault commits whole; a lost response is then
    // reconciled from the durable checkpoint, and the replay is a no-op.
    let committed = apply(&store, &batch, 6).unwrap();
    assert_eq!(committed.rows_inserted, 2);
    let after = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    assert_ne!(after, prior);
    drop(store);
    let store = open(dir.path());
    store
        .with_conn(|conn| {
            assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::Applied);
            assert_eq!(durable(conn), after);
            Ok(())
        })
        .unwrap();
    let replay = apply(&store, &batch, 7).unwrap();
    assert_eq!(
        replay,
        BatchOutcome {
            rows_inserted: 0,
            rows_replayed: 2,
            tombstones_recorded: 0,
            pending_created: 0,
            pending_obsoleted: 0,
            checkpoint_commit_seq: 4,
        }
    );
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), after);
            Ok(())
        })
        .unwrap();
}

#[test]
fn replay_and_old_prefixes_never_resurrect_tombstones_or_duplicate_work_and_conflicts_fail_closed()
{
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let sources = vec![
        Source {
            class: "messages",
            key: "a".into(),
            revision: 1,
            text: "a1".into(),
            created: 3,
        },
        Source {
            class: "messages",
            key: "b".into(),
            revision: 1,
            text: "b1".into(),
            created: 3,
        },
        Source {
            class: "raw_tool_spans",
            key: "raw".into(),
            revision: 1,
            text: "raw output".into(),
            created: 3,
        },
    ];
    let arena = Arena::new(&sources);
    let borrowed = borrow(&arena);
    let first_records = records(&sources, &arena, &borrowed);
    let a1 = occurrence_id(&first_records[0]);
    let batch1 = ProjectionBatch {
        identity: mutation(3, 3),
        records: first_records,
        invalidations: vec![],
        generation_id: Some(GENERATION),
    };
    apply(&store, &batch1, 1).unwrap();
    // a is superseded by revision 2 in the next window.
    let newer = vec![Source {
        class: "messages",
        key: "a".into(),
        revision: 2,
        text: "a2".into(),
        created: 5,
    }];
    let arena2 = Arena::new(&newer);
    let borrowed2 = borrow(&arena2);
    let batch2 = ProjectionBatch {
        identity: mutation(3, 5),
        records: records(&newer, &arena2, &borrowed2),
        invalidations: vec![Invalidation {
            occurrence_id: a1.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 5,
                reason: TombstoneReason::Superseded,
            },
        }],
        generation_id: Some(GENERATION),
    };
    apply(&store, &batch2, 2).unwrap();
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "UPDATE vector_generations SET state='retired' WHERE generation_id=?1",
                [GENERATION],
            )?;
            Ok(())
        })
        .unwrap();
    let settled = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    assert_eq!(
        settled.jobs[&(a1.clone(), GENERATION.to_string())],
        "obsolete"
    );

    // Replaying the old prefix that created a1 neither revives it nor queues
    // it again, and the checkpoint stays where it was.
    let replay = apply(&store, &batch1, 3).unwrap();
    assert_eq!(replay.rows_replayed, 3);
    assert_eq!(replay.pending_created, 0);
    assert_eq!(replay.checkpoint_commit_seq, 5);
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), settled, "the old prefix moved nothing");
            assert_eq!(
                batch_status(
                    conn,
                    &ProjectionBatch {
                        identity: mutation(3, 4),
                        ..batch1.clone()
                    }
                )
                .unwrap(),
                BatchStatus::Applied
            );
            Ok(())
        })
        .unwrap();
    // Overlapping window: the same rows as batch2 plus a1's creation again.
    let mut overlapping = batch2.clone();
    overlapping.identity = mutation(3, 6);
    overlapping.records.extend(batch1.records.iter().cloned());
    let outcome = apply(&store, &overlapping, 4).unwrap();
    assert_eq!(outcome.rows_inserted, 0);
    assert_eq!(outcome.pending_created, 0);
    assert_eq!(outcome.checkpoint_commit_seq, 6);
    let after = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    assert_eq!(after.tombstones, settled.tombstones);
    assert_eq!(after.jobs, settled.jobs);
    assert_eq!(after.pending, settled.pending);
    assert_eq!(after.checkpoint, Some((3, 6, HOLD.to_string())));

    // Conflicting mutation identities fail closed and change nothing.
    let mut other_hold = batch2.clone();
    other_hold.identity.hold_id = "hold-2".to_string();
    other_hold.identity.through_commit_seq = 7;
    assert_eq!(
        apply(&store, &other_hold, 5),
        Err(ProjectionError::MutationConflict)
    );
    let mut other_snapshot = batch2.clone();
    other_snapshot.identity.snapshot_commit_seq = 4;
    other_snapshot.identity.through_commit_seq = 7;
    assert_eq!(
        apply(&store, &other_snapshot, 5),
        Err(ProjectionError::MutationConflict)
    );
    let mut other_kernel = batch2.clone();
    other_kernel.identity.kernel_incarnation_id = "kernel-2".to_string();
    store
        .with_conn(|conn| {
            assert_eq!(
                batch_status(conn, &other_kernel),
                Err(ProjectionError::IdentityMismatch)
            );
            Ok(())
        })
        .unwrap();
    other_kernel.identity.through_commit_seq = 7;
    assert_eq!(
        apply(&store, &other_kernel, 5),
        Err(ProjectionError::IdentityMismatch)
    );
    let mut other_generation = batch2.clone();
    other_generation.identity.through_commit_seq = 7;
    other_generation.generation_id = Some("gen-unknown");
    assert_eq!(
        apply(&store, &other_generation, 5),
        Err(ProjectionError::UnknownGeneration {
            generation_id: "gen-unknown".to_string(),
        })
    );
    let mut backwards = batch2.clone();
    backwards.identity.through_commit_seq = 2;
    assert_eq!(
        apply(&store, &backwards, 5),
        Err(ProjectionError::MutationConflict)
    );
    let mut new_work = batch2.clone();
    new_work.identity.through_commit_seq = 7;
    new_work.records[0].occurrence.revision = "3";
    assert_eq!(
        store
            .with_conn_fenced(|conn| Ok(apply_batch(conn, &new_work, bounds(), 5)))
            .unwrap(),
        Err(ProjectionError::UnknownGeneration {
            generation_id: GENERATION.to_string(),
        }),
        "a retired generation cannot receive new work"
    );
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), after);
            let state: String = conn.query_row(
                "SELECT state FROM vector_generations WHERE generation_id=?1",
                [GENERATION],
                |row| row.get(0),
            )?;
            assert_eq!(state, "retired");
            Ok(())
        })
        .unwrap();
    // A generation registered again with another identity is refused too.
    store
        .with_conn_fenced(|conn| {
            let mut changed = generation();
            changed.vector_dimension = 16;
            assert_eq!(
                register_generation(conn, &changed, 9),
                Err(ProjectionError::IdentityMismatch)
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn an_oversized_first_commit_or_exhausted_pending_capacity_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let sources: Vec<Source> = (0..6)
        .map(|i| Source {
            class: "messages",
            key: format!("m{i}"),
            revision: 1,
            text: format!("message number {i}"),
            created: 2,
        })
        .collect();
    let arena = Arena::new(&sources);
    let borrowed = borrow(&arena);
    let batch = ProjectionBatch {
        identity: mutation(2, 2),
        records: records(&sources, &arena, &borrowed),
        invalidations: vec![],
        generation_id: Some(GENERATION),
    };
    let empty = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    assert!(empty.occurrences.is_empty() && empty.checkpoint.is_none());
    let refusals = [
        (
            BatchBounds {
                max_local_mutations: NonZeroUsize::new(5).unwrap(),
                ..bounds()
            },
            "local_mutations",
        ),
        (
            BatchBounds {
                max_source_bytes: NonZeroUsize::new(50).unwrap(),
                ..bounds()
            },
            "source_bytes",
        ),
        (
            BatchBounds {
                max_pending: NonZeroUsize::new(5).unwrap(),
                ..bounds()
            },
            "pending",
        ),
        (
            BatchBounds {
                persist: PersistBounds {
                    max_payload_bytes: NonZeroUsize::new(10).unwrap(),
                    ..bounds().persist
                },
                ..bounds()
            },
            "payload_bytes",
        ),
    ];
    for (tight, bound) in refusals {
        let mut outcome = None;
        let _ = store.with_conn_fenced(|conn| {
            outcome = Some(apply_batch(conn, &batch, tight, 1));
            Err::<(), _>(rusqlite::Error::QueryReturnedNoRows)
        });
        match outcome.unwrap() {
            Err(ProjectionError::BatchOverBound { bound: named, .. }) => assert_eq!(named, bound),
            Err(ProjectionError::OverBound { bound: named, .. }) => assert_eq!(named, bound),
            other => panic!("{bound}: {other:?}"),
        }
        store
            .with_conn(|conn| {
                assert_eq!(durable(conn), empty, "{bound}");
                let payloads: i64 =
                    conn.query_row("SELECT COUNT(*) FROM payloads", [], |r| r.get(0))?;
                assert_eq!(payloads, 0, "{bound}: no payload row precedes admission");
                Ok(())
            })
            .unwrap();
    }
    // Capacity is judged on the total: five pending already, one more batch of
    // one dense row over a bound of five is refused whole.
    apply(
        &store,
        &ProjectionBatch {
            records: batch.records[..5].to_vec(),
            ..batch.clone()
        },
        1,
    )
    .unwrap();
    let five = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    assert_eq!(five.pending.len(), 5);
    let sixth = ProjectionBatch {
        identity: mutation(2, 3),
        records: batch.records[5..].to_vec(),
        invalidations: vec![],
        generation_id: Some(GENERATION),
    };
    let mut outcome = None;
    let _ = store.with_conn_fenced(|conn| {
        outcome = Some(apply_batch(
            conn,
            &sixth,
            BatchBounds {
                max_pending: NonZeroUsize::new(5).unwrap(),
                ..bounds()
            },
            2,
        ));
        Err::<(), _>(rusqlite::Error::QueryReturnedNoRows)
    });
    assert_eq!(
        outcome.unwrap(),
        Err(ProjectionError::BatchOverBound {
            bound: "pending",
            size: 6
        })
    );
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), five);
            Ok(())
        })
        .unwrap();
    // Replaying the applied five-row batch at exactly that capacity is not a
    // refusal: its work is already queued and charges nothing new.
    let mut outcome = None;
    store
        .with_conn_fenced(|conn| {
            outcome = Some(apply_batch(
                conn,
                &ProjectionBatch {
                    records: batch.records[..5].to_vec(),
                    ..batch.clone()
                },
                BatchBounds {
                    max_pending: NonZeroUsize::new(5).unwrap(),
                    ..bounds()
                },
                2,
            ));
            Ok(())
        })
        .unwrap();
    let replay = outcome.unwrap().unwrap();
    assert_eq!((replay.rows_replayed, replay.pending_created), (5, 0));
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), five);
            Ok(())
        })
        .unwrap();
    // Without a generation the same rows land with no dense work at all.
    let lexical = ProjectionBatch {
        generation_id: None,
        ..sixth
    };
    let outcome = apply(&store, &lexical, 3).unwrap();
    assert_eq!((outcome.rows_inserted, outcome.pending_created), (1, 0));
    // A lexical-only batch still obsoletes queued work for what it tombstones.
    let first_id = five.pending.iter().next().unwrap().clone();
    let lexical_tombstone = ProjectionBatch {
        identity: mutation(2, 4),
        records: vec![],
        invalidations: vec![Invalidation {
            occurrence_id: first_id.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 4,
                reason: TombstoneReason::Retired,
            },
        }],
        generation_id: None,
    };
    let outcome = apply(&store, &lexical_tombstone, 4).unwrap();
    assert_eq!(
        (outcome.tombstones_recorded, outcome.pending_obsoleted),
        (1, 1)
    );
    store
        .with_conn(|conn| {
            let state = durable(conn);
            assert_eq!(
                state.jobs[&(first_id.clone(), GENERATION.to_string())],
                "obsolete"
            );
            assert!(!state.pending.contains(&first_id));
            Ok(())
        })
        .unwrap();
    // A tombstoned row re-presented with a generation queues no work, whether
    // or not it ever had a job: the sixth row never had one because it landed
    // without a generation.
    let sixth_id = occurrence_id(&batch.records[5]);
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(2, 5),
            records: vec![],
            invalidations: vec![Invalidation {
                occurrence_id: sixth_id.clone(),
                tombstone: Tombstone {
                    invalidated_commit_seq: 5,
                    reason: TombstoneReason::Retired,
                },
            }],
            generation_id: None,
        },
        5,
    )
    .unwrap();
    let outcome = apply(
        &store,
        &ProjectionBatch {
            identity: mutation(2, 6),
            records: batch.records[5..].to_vec(),
            invalidations: vec![],
            generation_id: Some(GENERATION),
        },
        6,
    )
    .unwrap();
    assert_eq!((outcome.rows_replayed, outcome.pending_created), (1, 0));
    store
        .with_conn(|conn| {
            assert!(
                !durable(conn)
                    .jobs
                    .contains_key(&(sixth_id.clone(), GENERATION.to_string())),
                "no job row for a tombstoned occurrence"
            );
            Ok(())
        })
        .unwrap();
    let final_state = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    drop(store);
    let store = open(dir.path());
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), final_state);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_whole_buffer_record_under_a_whole_covering_span_lands_under_the_spanless_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let sources = vec![Source {
        class: "messages",
        key: "m1".into(),
        revision: 1,
        text: "hello".into(),
        created: 2,
    }];
    let arena = Arena::new(&sources);
    let borrowed = borrow(&arena);
    let spanless = records(&sources, &arena, &borrowed);
    let spanless_id = occurrence_id(&spanless[0]);
    let spanned: Vec<OccurrenceRecord<'_>> = spanless
        .iter()
        .map(|record| OccurrenceRecord {
            occurrence: Occurrence {
                span: Some(Span {
                    start: 0,
                    end: sources[0].text.len() as u64,
                }),
                ..record.occurrence
            },
            ..record.clone()
        })
        .collect();
    assert_ne!(
        occurrence_id(&spanned[0]),
        spanless_id,
        "the un-normalized spellings encode differently"
    );
    let batch = ProjectionBatch {
        identity: mutation(2, 2),
        records: spanned,
        invalidations: vec![],
        generation_id: Some(GENERATION),
    };
    let outcome = apply(&store, &batch, 1).expect("a whole-covering span is a valid record");
    assert_eq!((outcome.rows_inserted, outcome.pending_created), (1, 1));
    store
        .with_conn(|conn| {
            assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::Applied);
            let stored = read_occurrence(conn, &spanless_id)
                .unwrap()
                .expect("stored under the spanless identity");
            assert_eq!(stored.span, None);
            assert_eq!(stored.bytes, b"hello");
            let state = durable(conn);
            assert_eq!(state.occurrences.len(), 1);
            assert_eq!(
                state.pending.iter().collect::<Vec<_>>(),
                [&spanless_id],
                "the job is keyed by the identity the row landed under"
            );
            Ok(())
        })
        .unwrap();
    let replay = apply(
        &store,
        &ProjectionBatch {
            records: spanless,
            ..batch.clone()
        },
        2,
    )
    .unwrap();
    assert_eq!(
        (
            replay.rows_inserted,
            replay.rows_replayed,
            replay.pending_created
        ),
        (0, 1, 0),
        "the spanless spelling replays onto the same row"
    );
}

#[test]
fn the_pending_charge_counts_only_work_the_batch_will_queue() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let at_capacity = BatchBounds {
        max_pending: NonZeroUsize::new(5).unwrap(),
        ..bounds()
    };
    let apply_bounded = |batch: &ProjectionBatch<'_>, now: i64| {
        let mut outcome = None;
        let _ = store.with_conn_fenced(|conn| {
            let result = apply_batch(conn, batch, at_capacity, now);
            let failed = result.is_err();
            outcome = Some(result);
            if failed {
                Err(rusqlite::Error::QueryReturnedNoRows)
            } else {
                Ok(())
            }
        });
        outcome.unwrap()
    };
    let sources: Vec<Source> = (0..7)
        .map(|i| Source {
            class: "messages",
            key: format!("m{i}"),
            revision: 1,
            text: format!("message number {i}"),
            created: 2,
        })
        .collect();
    let arena = Arena::new(&sources);
    let borrowed = borrow(&arena);
    let all = records(&sources, &arena, &borrowed);
    let ids: Vec<String> = all.iter().map(|r| occurrence_id(r)).collect();
    apply_bounded(
        &ProjectionBatch {
            identity: mutation(2, 2),
            records: all[..5].to_vec(),
            invalidations: vec![],
            generation_id: Some(GENERATION),
        },
        1,
    )
    .unwrap();
    let full = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    assert_eq!(full.pending.len(), 5, "pending capacity is exactly filled");
    let born_dead = ProjectionBatch {
        identity: mutation(2, 3),
        records: all[5..6].to_vec(),
        invalidations: vec![Invalidation {
            occurrence_id: ids[5].clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 3,
                reason: TombstoneReason::Retired,
            },
        }],
        generation_id: Some(GENERATION),
    };
    let outcome =
        apply_bounded(&born_dead, 2).expect("a row tombstoned in its own batch charges nothing");
    assert_eq!(
        (
            outcome.rows_inserted,
            outcome.tombstones_recorded,
            outcome.pending_created
        ),
        (1, 1, 0)
    );
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn).pending, full.pending);
            Ok(())
        })
        .unwrap();
    let swap = ProjectionBatch {
        identity: mutation(2, 4),
        records: vec![all[6].clone(), all[6].clone(), all[1].clone()],
        invalidations: vec![Invalidation {
            occurrence_id: ids[0].clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 4,
                reason: TombstoneReason::Retired,
            },
        }],
        generation_id: Some(GENERATION),
    };
    let outcome = apply_bounded(&swap, 3).expect("work the batch obsoletes is credited");
    assert_eq!((outcome.pending_created, outcome.pending_obsoleted), (1, 1));
    assert_eq!((outcome.rows_inserted, outcome.rows_replayed), (1, 2));
    store
        .with_conn(|conn| {
            let state = durable(conn);
            assert_eq!(state.pending.len(), 5);
            assert!(!state.pending.contains(&ids[0]));
            assert!(state.pending.contains(&ids[6]));
            Ok(())
        })
        .unwrap();
    let over = ProjectionBatch {
        identity: mutation(2, 5),
        records: all[5..6]
            .iter()
            .map(|record| OccurrenceRecord {
                occurrence: Occurrence {
                    revision: "2",
                    ..record.occurrence
                },
                ..record.clone()
            })
            .collect(),
        invalidations: vec![],
        generation_id: Some(GENERATION),
    };
    assert_eq!(
        apply_bounded(&over, 4),
        Err(ProjectionError::BatchOverBound {
            bound: "pending",
            size: 6
        }),
        "a new dense row with nothing obsoleted is still over the bound"
    );
}

/// An exported row as the kernel would hand it back, built by hand so the
/// window edges and the selected-payload path are pinned without a kernel.
fn exported_row(
    key: &str,
    revision: i64,
    text: Option<&str>,
    span: Option<(u64, u64)>,
    created: i64,
    invalidated: Option<i64>,
    superseded_by: Option<&str>,
) -> kernel::SourceRow {
    let source = Source {
        class: "messages",
        key: key.to_string(),
        revision,
        text: text.unwrap_or_default().to_string(),
        created,
    };
    let identity = source.identity();
    let borrowed: Vec<(&str, &str)> = identity
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let revision_text = revision.to_string();
    let encoded = kernel::source_identity::encode_preserving_span(&Occurrence {
        class: "messages",
        identity: &borrowed,
        revision: &revision_text,
        representation: "text",
        span: span.map(|(start, end)| kernel::source_identity::Span { start, end }),
    })
    .unwrap();
    kernel::SourceRow {
        object_id: format!("srcdesc:{}:{revision}", encoded.lineage_id),
        revision,
        detail: kernel::SourceDescriptorDetail {
            descriptor_version: kernel::SOURCE_DESCRIPTOR_DETAIL_VERSION,
            source_policy: kernel::SourceDescriptorPolicy::Native,
            class: "messages".to_string(),
            identity,
            revision: revision_text,
            representation: "text".to_string(),
            span,
            occurrence_tuple: encoded.tuple,
            occurrence_id: encoded.occurrence_id,
            lineage_id: encoded.lineage_id,
            payload_id: kernel::source_identity::payload_id(text.unwrap_or_default().as_bytes()),
            artifact_digest: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            evidence_id: format!("evidence-{key}"),
        },
        domain_id: "domain".to_string(),
        sensitivity: Sensitivity::Normal,
        created_commit_seq: created,
        invalidated_commit_seq: invalidated,
        superseded_by: superseded_by.map(str::to_string),
        text: text.map(str::to_string),
    }
}

#[test]
fn recovery_authorizations_remember_all_consumed_references_across_reopen() {
    use retrieval::batch::{batch_from_rows, row_identities};
    use retrieval::dispatch::{
        Admission, Disposition, EpisodeGrant, LaneBinding, Recovery, authorize_recovery,
        charge_admission, dispatch_jobs, eligible_job_candidates, job_ledger, record_retry,
        stop_job,
    };
    use std::num::NonZeroU32;

    let dir = tempfile::tempdir().unwrap();
    let mut store = open(dir.path());
    setup(&store);
    let rows =
        ["first", "second"].map(|key| exported_row(key, 1, Some("input"), None, 3, None, None));
    let identities = row_identities(&rows);
    let batch = batch_from_rows(&rows, &identities, mutation(3, 3), Some(GENERATION)).unwrap();
    let grant = EpisodeGrant {
        allowance: NonZeroU32::new(1).unwrap(),
        deadline: 100,
    };
    let host = LaneBinding {
        embedding_model: "model-a".to_owned(),
        bundle_fingerprint: "fp-a".to_owned(),
        vector_dimension: 8,
        table_epoch: 1,
        host_incarnation: "host".to_owned(),
    };
    let jobs = store
        .with_conn_fenced(|conn| {
            apply_batch(conn, &batch, bounds(), 3).unwrap();
            let candidates =
                eligible_job_candidates(conn, None, NonZeroUsize::new(2).unwrap(), 3).unwrap();
            let job_ids: Vec<_> = candidates
                .iter()
                .map(|candidate| candidate.job_id.clone())
                .collect();
            let jobs = dispatch_jobs(conn, &job_ids, 3).unwrap();
            assert_eq!(jobs.len(), 2);
            assert_eq!(
                authorize_recovery(conn, "unknown", "A", grant, 4).unwrap(),
                Recovery::NotStopped
            );
            assert_eq!(
                authorize_recovery(conn, &jobs[0].job_id, "A", grant, 4).unwrap(),
                Recovery::NotStopped
            );
            for job in &jobs {
                assert!(stop_job(conn, &job.job_id, "input", 4).unwrap());
            }
            Ok(jobs)
        })
        .unwrap();
    let job_id = &jobs[0].job_id;
    let before = store
        .with_conn_fenced(|conn| Ok(job_ledger(conn, job_id).unwrap()))
        .unwrap();
    let rolled_back: Result<(), _> = store.with_conn_fenced(|conn| {
        assert!(matches!(
            authorize_recovery(conn, job_id, "A", grant, 5).unwrap(),
            Recovery::Granted { .. }
        ));
        Err(rusqlite::Error::QueryReturnedNoRows)
    });
    assert!(rolled_back.is_err());
    drop(store);
    store = open(dir.path());
    assert_eq!(
        store
            .with_conn_fenced(|conn| Ok(job_ledger(conn, job_id).unwrap()))
            .unwrap(),
        before
    );

    for authorization in ["A", "B"] {
        store
            .with_conn_fenced(|conn| {
                assert_eq!(
                    authorize_recovery(conn, job_id, authorization, grant, 5).unwrap(),
                    Recovery::Granted {
                        episode_id: format!("{job_id}/auth/{authorization}")
                    }
                );
                assert_eq!(
                    charge_admission(conn, job_id, &host, authorization, grant, 6).unwrap(),
                    Admission::Charged { attempts: 1 }
                );
                assert_eq!(
                    record_retry(conn, job_id, "execution_failure", 7, 6).unwrap(),
                    Disposition::Exhausted
                );
                Ok(())
            })
            .unwrap();
        drop(store);
        store = open(dir.path());
    }
    store
        .with_conn_fenced(|conn| {
            let stopped = job_ledger(conn, job_id).unwrap().unwrap();
            assert_eq!((stopped.state.as_str(), stopped.attempts), ("failed", 1));
            assert_eq!(stopped.authorization_ref.as_deref(), Some("B"));
            let larger = EpisodeGrant {
                allowance: NonZeroU32::new(9).unwrap(),
                deadline: 1000,
            };
            assert_eq!(
                authorize_recovery(conn, job_id, "A", larger, 8).unwrap(),
                Recovery::Replayed,
                "A/B/A must not replenish the stopped B episode"
            );
            assert_eq!(job_ledger(conn, job_id).unwrap().unwrap(), stopped);
            assert!(
                eligible_job_candidates(conn, None, NonZeroUsize::new(2).unwrap(), 8)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                matches!(
                    authorize_recovery(conn, &jobs[1].job_id, "A", grant, 8).unwrap(),
                    Recovery::Granted { .. }
                ),
                "references are scoped to one job"
            );
            assert!(matches!(
                authorize_recovery(conn, job_id, "C", grant, 8).unwrap(),
                Recovery::Granted { .. }
            ));
            let pending = job_ledger(conn, job_id).unwrap();
            assert_eq!(
                authorize_recovery(conn, job_id, "A", larger, 9).unwrap(),
                Recovery::Replayed,
                "consumed history is checked before the non-stopped state"
            );
            assert_eq!(job_ledger(conn, job_id).unwrap(), pending);
            Ok(())
        })
        .unwrap();
}

#[test]
fn dispatch_jobs_reject_same_length_utf8_payload_corruption() {
    use retrieval::batch::{batch_from_rows, row_identities};
    use retrieval::dispatch::{dispatch_jobs, eligible_job_candidates};

    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let rows = [exported_row(
        "integrity",
        1,
        Some("hello"),
        None,
        3,
        None,
        None,
    )];
    let identities = row_identities(&rows);
    let batch = batch_from_rows(&rows, &identities, mutation(3, 3), Some(GENERATION)).unwrap();
    store
        .with_conn_fenced(|conn| {
            apply_batch(conn, &batch, bounds(), 3).unwrap();
            let candidates =
                eligible_job_candidates(conn, None, NonZeroUsize::new(1).unwrap(), 3).unwrap();
            let job_ids = [candidates[0].job_id.clone()];
            let jobs = dispatch_jobs(conn, &job_ids, 3).unwrap();
            assert_eq!(jobs[0].text, "hello");
            conn.execute("UPDATE payloads SET bytes=?1", [b"jello".as_slice()])?;
            assert!(matches!(
                dispatch_jobs(conn, &job_ids, 3),
                Err(ProjectionError::CorruptRow)
            ));
            Ok(())
        })
        .unwrap();
}

#[test]
fn selected_job_that_closes_before_hydration_is_skipped() {
    use retrieval::batch::{batch_from_rows, row_identities};
    use retrieval::dispatch::{dispatch_jobs, eligible_job_candidates};

    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let rows = [exported_row(
        "hydrate-race",
        1,
        Some("hello"),
        None,
        3,
        None,
        None,
    )];
    let identities = row_identities(&rows);
    let batch = batch_from_rows(&rows, &identities, mutation(3, 3), Some(GENERATION)).unwrap();
    store
        .with_conn_fenced(|conn| {
            apply_batch(conn, &batch, bounds(), 3).unwrap();
            let candidates =
                eligible_job_candidates(conn, None, NonZeroUsize::new(1).unwrap(), 3).unwrap();
            let job_ids = [candidates[0].job_id.clone()];
            conn.execute(
                "UPDATE embedding_jobs SET state='obsolete' WHERE job_id=?1",
                [&job_ids[0]],
            )?;
            assert!(dispatch_jobs(conn, &job_ids, 3).unwrap().is_empty());
            conn.execute("DELETE FROM embedding_jobs WHERE job_id=?1", [&job_ids[0]])?;
            assert!(dispatch_jobs(conn, &job_ids, 3).unwrap().is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn selected_job_with_a_corrupt_generation_relationship_is_rejected() {
    use retrieval::batch::{batch_from_rows, row_identities};
    use retrieval::dispatch::{dispatch_jobs, eligible_job_candidates};

    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let rows = [exported_row(
        "hydrate-corrupt",
        1,
        Some("hello"),
        None,
        3,
        None,
        None,
    )];
    let identities = row_identities(&rows);
    let batch = batch_from_rows(&rows, &identities, mutation(3, 3), Some(GENERATION)).unwrap();
    let rolled_back: Result<(), _> = store.with_conn_fenced(|conn| {
        apply_batch(conn, &batch, bounds(), 3).unwrap();
        let candidates =
            eligible_job_candidates(conn, None, NonZeroUsize::new(1).unwrap(), 3).unwrap();
        let job_ids = [candidates[0].job_id.clone()];
        conn.execute("PRAGMA defer_foreign_keys=ON", [])?;
        conn.execute(
            "UPDATE embedding_jobs SET generation_id='missing' WHERE job_id=?1",
            [&job_ids[0]],
        )?;
        assert!(matches!(
            dispatch_jobs(conn, &job_ids, 3),
            Err(ProjectionError::CorruptRow)
        ));
        Err(rusqlite::Error::QueryReturnedNoRows)
    });
    assert!(rolled_back.is_err());
}

#[test]
fn dispatch_job_debug_omits_text_and_reports_utf8_bytes() {
    let job = retrieval::dispatch::DispatchJob {
        job_id: "debug-job-identity".to_owned(),
        occurrence_id: "occurrence".to_owned(),
        generation: generation(),
        payload_id: "payload".to_owned(),
        source_object_id: "source".to_owned(),
        revision: 1,
        source_artifact_digest: "digest".to_owned(),
        text: "private-dispatch-sentinel-雪".to_owned(),
        state: "pending".to_owned(),
        attempts: 0,
        episode: None,
        host_job_id: None,
    };
    for debug in [format!("{job:?}"), format!("{job:#?}")] {
        assert!(
            !debug.contains("private-dispatch-sentinel"),
            "Debug must not expose payload text"
        );
        assert!(
            debug.contains(&format!("text_bytes: {}", job.text.len())),
            "Debug must report UTF-8 byte length"
        );
        assert!(debug.contains(&format!("job_id: {:?}", job.job_id)));
    }
}

#[test]
fn batch_status_requires_the_requested_generation_and_batch_effects() {
    use retrieval::batch::{batch_from_rows, row_identities};

    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let rows = vec![exported_row(
        "status",
        1,
        Some("hello"),
        Some((7, 12)),
        2,
        None,
        None,
    )];
    let identities = row_identities(&rows);
    let batch = batch_from_rows(&rows, &identities, mutation(2, 2), Some(GENERATION)).unwrap();
    assert_eq!(
        store
            .with_conn(|conn| Ok(batch_status(conn, &batch)))
            .unwrap(),
        Err(ProjectionError::IdentityMismatch)
    );
    setup(&store);
    let mut statuses = vec![
        store
            .with_conn(|conn| Ok(batch_status(conn, &batch).unwrap()))
            .unwrap(),
    ];
    let empty = ProjectionBatch {
        records: vec![],
        generation_id: None,
        ..batch.clone()
    };
    apply(&store, &empty, 2).unwrap();
    statuses.push(
        store
            .with_conn(|conn| Ok(batch_status(conn, &batch).unwrap()))
            .unwrap(),
    );
    store
        .with_conn_fenced(|conn| {
            register_generation(
                conn,
                &VectorGeneration {
                    generation_id: "gen-other".to_string(),
                    ..generation()
                },
                2,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    for generation_id in [None, Some("gen-other")] {
        let other = ProjectionBatch {
            generation_id,
            ..batch.clone()
        };
        apply(&store, &other, 3).unwrap();
        statuses.push(
            store
                .with_conn(|conn| Ok(batch_status(conn, &batch).unwrap()))
                .unwrap(),
        );
        assert_eq!(
            store
                .with_conn(|conn| Ok(batch_status(conn, &other).unwrap()))
                .unwrap(),
            BatchStatus::Applied
        );
    }
    assert_eq!(
        statuses,
        [BatchStatus::NotApplied; 4],
        "an absent checkpoint, empty batch, lexical batch, or other generation cannot satisfy the requested batch"
    );
    assert_eq!(apply(&store, &batch, 4).unwrap().pending_created, 1);
    drop(store);
    let store = open(dir.path());
    store
        .with_conn(|conn| {
            assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::Applied);
            Ok(())
        })
        .unwrap();
    for state in ["admitted", "embedded", "published", "failed", "obsolete"] {
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE embedding_jobs SET state=?1 WHERE generation_id=?2",
                    [state, GENERATION],
                )?;
                Ok(())
            })
            .unwrap();
        store
            .with_conn(|conn| {
                assert_eq!(
                    batch_status(conn, &batch).unwrap(),
                    BatchStatus::Applied,
                    "{state}"
                );
                Ok(())
            })
            .unwrap();
    }
    store
        .with_conn_fenced(|conn| {
            conn.execute("UPDATE vector_generations SET state='retired'", [])?;
            Ok(())
        })
        .unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::Applied);
            Ok(())
        })
        .unwrap();
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "DELETE FROM embedding_jobs WHERE generation_id=?1",
                [GENERATION],
            )?;
            Ok(())
        })
        .unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                batch_status(conn, &batch).unwrap(),
                BatchStatus::NotApplied,
                "retiring a generation does not prove its missing job was applied"
            );
            Ok(())
        })
        .unwrap();
    let invalidation = ProjectionBatch {
        identity: mutation(2, 3),
        records: vec![],
        invalidations: vec![Invalidation {
            occurrence_id: rows[0].detail.occurrence_id.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 3,
                reason: TombstoneReason::Retired,
            },
        }],
        generation_id: None,
    };
    apply(
        &store,
        &ProjectionBatch {
            identity: mutation(2, 3),
            ..empty
        },
        5,
    )
    .unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                batch_status(conn, &invalidation).unwrap(),
                BatchStatus::NotApplied
            );
            Ok(())
        })
        .unwrap();
    store
        .with_conn_fenced(|conn| {
            retrieval::tombstone_occurrence(
                conn,
                &invalidation.invalidations[0].occurrence_id,
                invalidation.invalidations[0].tombstone,
                6,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    for state in ["pending", "admitted"] {
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE embedding_jobs SET state=?1 WHERE generation_id='gen-other'",
                    [state],
                )?;
                Ok(())
            })
            .unwrap();
        store
            .with_conn(|conn| {
                assert_eq!(
                    batch_status(conn, &invalidation).unwrap(),
                    BatchStatus::NotApplied,
                    "the tombstone does not prove {state} work was obsoleted"
                );
                Ok(())
            })
            .unwrap();
    }
    apply(&store, &invalidation, 6).unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                batch_status(conn, &invalidation).unwrap(),
                BatchStatus::Applied
            );
            assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::Applied);
            Ok(())
        })
        .unwrap();
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "UPDATE projection_identity SET kernel_incarnation_id='other'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                batch_status(conn, &batch),
                Err(ProjectionError::IdentityMismatch)
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn rows_map_to_a_batch_at_the_window_edges_and_selected_text_persists_under_its_span() {
    use retrieval::batch::{batch_from_rows, row_identities};
    let snapshot = 10;
    let through = 20;
    // Invalidations at S, S+1, through, and through+1; a row created and
    // superseded inside the window; a spanned selection.
    let rows = vec![
        exported_row("at-s", 1, None, None, 3, Some(10), None),
        exported_row("after-s", 1, None, None, 3, Some(11), Some("newer")),
        exported_row("at-through", 1, None, None, 4, Some(20), None),
        exported_row("after-through", 1, None, None, 4, Some(21), None),
        exported_row(
            "both",
            1,
            Some("created then superseded"),
            None,
            12,
            Some(15),
            Some("n"),
        ),
        exported_row(
            "spanned",
            1,
            Some("selected part"),
            Some((7, 20)),
            13,
            None,
            None,
        ),
    ];
    let identities = row_identities(&rows);
    let batch = batch_from_rows(
        &rows,
        &identities,
        mutation(snapshot, through),
        Some(GENERATION),
    )
    .unwrap();
    let invalidated: Vec<(&str, &str)> = batch
        .invalidations
        .iter()
        .map(|i| {
            let key = rows
                .iter()
                .find(|r| r.detail.occurrence_id == i.occurrence_id)
                .map(|r| r.detail.evidence_id.as_str())
                .unwrap();
            (
                key,
                match i.tombstone.reason {
                    TombstoneReason::Superseded => "superseded",
                    TombstoneReason::Retired => "retired",
                    _ => "other",
                },
            )
        })
        .collect();
    assert_eq!(
        invalidated,
        vec![
            ("evidence-after-s", "superseded"),
            ("evidence-at-through", "retired"),
            ("evidence-both", "superseded"),
        ],
        "half-open window (S, through], reason from supersession"
    );
    assert_eq!(
        batch.records.len(),
        2,
        "the both-row and the spanned row carry text"
    );
    let short = &identities[..5];
    assert_eq!(
        batch_from_rows(&rows, short, mutation(snapshot, through), None).unwrap_err(),
        ProjectionError::MalformedBatch
    );
    let mut long = identities.clone();
    long.push(vec![]);
    assert_eq!(
        batch_from_rows(&rows, &long, mutation(snapshot, through), None).unwrap_err(),
        ProjectionError::MalformedBatch
    );
    for index in [1, 4] {
        let mut swapped = identities.clone();
        swapped.swap(index, index + 1);
        assert_eq!(
            batch_from_rows(&rows, &swapped, mutation(snapshot, through), None).err(),
            Some(ProjectionError::MalformedBatch),
            "borrowed identity belongs to another row at {index}"
        );
        let mut malformed = rows.clone();
        malformed[index].detail.occurrence_id = "not-an-occurrence-id".to_string();
        let borrowed = row_identities(&malformed);
        assert_eq!(
            batch_from_rows(&malformed, &borrowed, mutation(snapshot, through), None).err(),
            Some(ProjectionError::MalformedBatch),
            "descriptor identity must match its encoded fields at {index}"
        );
    }
    let mut corrupted_text = rows.clone();
    corrupted_text[5].text = Some("different txt".to_string());
    assert_eq!(corrupted_text[5].text.as_ref().unwrap().len(), 13);
    let mut corrupted_payload_id = rows.clone();
    corrupted_payload_id[5].detail.payload_id =
        kernel::source_identity::payload_id(b"other payload");
    let refusals: Vec<_> = [corrupted_text, corrupted_payload_id]
        .iter()
        .map(|rows| {
            batch_from_rows(
                rows,
                &row_identities(rows),
                mutation(snapshot, through),
                None,
            )
            .err()
        })
        .collect();
    assert_eq!(
        refusals,
        vec![Some(ProjectionError::MalformedBatch); 2],
        "selected bytes must match the descriptor payload identity"
    );

    // Selected text persists under the kernel's own occurrence identity and
    // span; a selection whose length disagrees with its span is refused
    // before anything is written.
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    // The rows live at S land first, as the fixed-S export would hand them.
    let at_s: Vec<kernel::SourceRow> = ["at-s", "after-s", "at-through", "after-through"]
        .iter()
        .map(|key| exported_row(key, 1, Some(&format!("text {key}")), None, 3, None, None))
        .collect();
    let at_s_identities = row_identities(&at_s);
    let snapshot_batch = batch_from_rows(
        &at_s,
        &at_s_identities,
        mutation(snapshot, snapshot),
        Some(GENERATION),
    )
    .unwrap();
    assert_eq!(apply(&store, &snapshot_batch, 0).unwrap().rows_inserted, 4);
    let outcome = apply(&store, &batch, 1).unwrap();
    assert_eq!(
        (
            outcome.rows_inserted,
            outcome.tombstones_recorded,
            outcome.pending_created,
            outcome.pending_obsoleted
        ),
        (2, 3, 1, 2)
    );
    store
        .with_conn(|conn| {
            let spanned = read_occurrence(conn, &rows[5].detail.occurrence_id)
                .unwrap()
                .expect("stored under the exported occurrence id");
            assert_eq!(spanned.span, Some((7, 20)));
            assert_eq!(spanned.bytes, b"selected part");
            let both = read_occurrence(conn, &rows[4].detail.occurrence_id)
                .unwrap()
                .unwrap();
            assert_eq!(both.tombstone.map(|t| t.invalidated_commit_seq), Some(15));
            assert!(
                !durable(conn)
                    .pending
                    .contains(&rows[4].detail.occurrence_id)
            );
            Ok(())
        })
        .unwrap();
    let before = store.with_conn(|conn| Ok(durable(conn))).unwrap();
    let mismatched = vec![exported_row(
        "cut",
        1,
        Some("too short"),
        Some((0, 100)),
        14,
        None,
        None,
    )];
    let identities = row_identities(&mismatched);
    let bad = batch_from_rows(&mismatched, &identities, mutation(snapshot, 21), None).unwrap();
    assert_eq!(
        apply(&store, &bad, 2),
        Err(ProjectionError::Occurrence(
            kernel::source_identity::OccurrenceRefusal::SpanOutOfRange
        ))
    );
    store
        .with_conn(|conn| {
            assert_eq!(durable(conn), before);
            Ok(())
        })
        .unwrap();
}
