//! `message_cleanup` against the projection alone: candidates page by occurrence id and name only tombstoned `messages` rows at or below the smaller of the acknowledged prefix and the projection checkpoint with no job row; `reclaim` re-checks each candidate inside the transaction, so a row that gained a job, a moved tombstone, or a higher prefix between the two observations survives; a payload shared with a live row survives its twin.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::Path;

use kernel::Sensitivity;
use kernel::source_identity::Occurrence;
use retrieval::batch::{
    BatchBounds, Invalidation, MutationIdentity, ProjectionBatch, VectorGeneration, apply_batch,
    register_generation,
};
use retrieval::message_cleanup::{Candidate, candidates, reclaim};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity, Tombstone,
    TombstoneReason, install_identity,
};
use rusqlite::params;
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

const HOLD: &str = "hold-1";
const KERNEL: &str = "kernel-1";
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

fn bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    }
}

fn mutation(through: i64) -> MutationIdentity {
    MutationIdentity {
        kernel_incarnation_id: KERNEL.to_string(),
        hold_id: HOLD.to_string(),
        snapshot_commit_seq: 1,
        through_commit_seq: through,
    }
}

/// One source the ledger owns: a message or a claim keyed by `key`.
struct Source {
    class: &'static str,
    key: &'static str,
    text: &'static str,
}

impl Source {
    fn identity(&self) -> Vec<(&'static str, String)> {
        match self.class {
            "messages" => vec![
                ("project_id", "proj-a".to_string()),
                ("harness", "opencode".to_string()),
                ("session_id", "sess-01".to_string()),
                ("message_id", format!("msg-{}", self.key)),
                ("block_index", "0".to_string()),
            ],
            "canonical_claims" => vec![("object_id", format!("obj-{}", self.key))],
            other => panic!("{other}"),
        }
    }

    fn representation(&self) -> &'static str {
        match self.class {
            "messages" => "text",
            _ => "decision_summary",
        }
    }
}

fn occurrence_id(source: &Source) -> String {
    let identity = source.identity();
    let borrowed: Vec<(&str, &str)> = identity
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    kernel::source_identity::encode_preserving_span(&Occurrence {
        class: source.class,
        identity: &borrowed,
        revision: "1",
        representation: source.representation(),
        span: None,
    })
    .unwrap()
    .occurrence_id
}

/// Applies one batch creating `sources` at commit 2, then one batch at `through` tombstoning `tombstoned`.
fn seed(store: &SqliteStore, sources: &[Source], tombstoned: &[&Source], through: i64) {
    store
        .with_conn_fenced(|conn| {
            install_identity(
                conn,
                &ProjectionIdentity {
                    schema_version: retrieval::SCHEMA_VERSION,
                    kernel_incarnation_id: KERNEL.to_string(),
                    projection_policy_version: "source-policy.v1".to_string(),
                    identity_contract_version: "search-projection-identity-v2".to_string(),
                    limit_manifest_protocol_version: "limits.v1".to_string(),
                    embedding_model: "model-a".to_string(),
                    tokenizer_fingerprint: "fp-a".to_string(),
                    vector_dimension: 8,
                    generation_epoch: 1,
                },
                1,
            )
            .unwrap();
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
            .unwrap();
            let identities: Vec<Vec<(&str, String)>> =
                sources.iter().map(Source::identity).collect();
            let borrowed: Vec<Vec<(&str, &str)>> = identities
                .iter()
                .map(|identity| identity.iter().map(|(n, v)| (*n, v.as_str())).collect())
                .collect();
            let records: Vec<OccurrenceRecord<'_>> = sources
                .iter()
                .zip(&borrowed)
                .map(|(source, identity)| OccurrenceRecord {
                    occurrence: Occurrence {
                        class: source.class,
                        identity,
                        revision: "1",
                        representation: source.representation(),
                        span: None,
                    },
                    payload: Payload::Whole(source.text),
                    domain_id: "domain",
                    sensitivity: Sensitivity::Normal,
                    source_object_id: source.key,
                    source_evidence_id: source.key,
                    source_artifact_digest:
                        "0000000000000000000000000000000000000000000000000000000000000000",
                    created_commit_seq: 2,
                })
                .collect();
            apply_batch(
                conn,
                &ProjectionBatch {
                    identity: mutation(2),
                    records,
                    invalidations: Vec::new(),
                    generation_id: Some(GENERATION),
                },
                bounds(),
                1,
            )
            .unwrap();
            apply_batch(
                conn,
                &ProjectionBatch {
                    identity: mutation(through),
                    records: Vec::new(),
                    invalidations: tombstoned
                        .iter()
                        .map(|source| Invalidation {
                            occurrence_id: occurrence_id(source),
                            tombstone: Tombstone {
                                invalidated_commit_seq: through,
                                reason: TombstoneReason::Retired,
                            },
                        })
                        .collect(),
                    generation_id: Some(GENERATION),
                },
                bounds(),
                2,
            )
            .unwrap();
            // The identity sweep would remove the obsolete job rows before cleanup; do so here for every tombstoned row.
            for source in tombstoned {
                conn.execute(
                    "DELETE FROM embedding_jobs WHERE occurrence_id=?1",
                    [occurrence_id(source)],
                )?;
            }
            Ok(())
        })
        .unwrap();
}

fn present(store: &SqliteStore) -> BTreeSet<String> {
    store
        .with_conn(|conn| {
            conn.prepare("SELECT occurrence_id FROM occurrences")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()
        })
        .unwrap()
}

#[test]
fn candidates_page_in_order_and_reclaim_rechecks_every_row_inside_the_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = Source {
        class: "messages",
        key: "a",
        text: "shared",
    };
    let b = Source {
        class: "messages",
        key: "b",
        text: "shared",
    };
    let c = Source {
        class: "messages",
        key: "c",
        text: "own",
    };
    let live = Source {
        class: "messages",
        key: "live",
        text: "live",
    };
    let claim = Source {
        class: "canonical_claims",
        key: "k",
        text: "claim",
    };
    let sources = [a, b, c, live, claim];
    let [a, b, c, live, claim] = &sources;
    // Tombstone a, c, and the claim at commit 5; b at commit 5 as well but it keeps a job row; live stays live.
    seed(&store, &sources, &[a, b, c, claim], 5);
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,attempts,created_at,updated_at)
                 VALUES ('held',?1,?2,'admitted',1,1,1)",
                params![occurrence_id(b), GENERATION],
            )?;
            Ok(())
        })
        .unwrap();
    let mut expected: Vec<String> = vec![occurrence_id(a), occurrence_id(c)];
    expected.sort();

    // A projection installed under another kernel incarnation fails closed on both observations: its sequences are not comparable to the caller's prefix.
    let (mismatched_selection, mismatched_reclaim) = store
        .with_conn_fenced(|conn| {
            Ok((
                candidates(conn, "kernel-2", 1_000, None, NonZeroUsize::new(8).unwrap()),
                reclaim(
                    conn,
                    "kernel-2",
                    &[Candidate {
                        occurrence_id: occurrence_id(a),
                        invalidated_commit_seq: 5,
                    }],
                    1_000,
                ),
            ))
        })
        .unwrap();
    assert!(
        matches!(mismatched_selection, Err(ProjectionError::IdentityMismatch)),
        "{mismatched_selection:?}"
    );
    assert!(
        matches!(mismatched_reclaim, Err(ProjectionError::IdentityMismatch)),
        "{mismatched_reclaim:?}"
    );
    assert_eq!(present(&store).len(), 5);

    // A prefix below the tombstones names nothing; the checkpoint caps a prefix above it.
    let (none, all, capped) = store
        .with_conn(|conn| {
            Ok((
                candidates(conn, KERNEL, 4, None, NonZeroUsize::new(8).unwrap()).unwrap(),
                candidates(conn, KERNEL, 5, None, NonZeroUsize::new(8).unwrap()).unwrap(),
                candidates(conn, KERNEL, 1_000, None, NonZeroUsize::new(8).unwrap()).unwrap(),
            ))
        })
        .unwrap();
    assert!(none.candidates.is_empty());
    assert_eq!(
        none.inspected, 3,
        "the three tombstoned message rows were visited"
    );
    let ids = |page: &retrieval::message_cleanup::CandidatePage| {
        page.candidates
            .iter()
            .map(|candidate| candidate.occurrence_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&all),
        expected,
        "b is held by its job row; the claim is another class"
    );
    assert_eq!(ids(&capped), expected);
    assert!(
        all.candidates
            .iter()
            .all(|candidate| candidate.invalidated_commit_seq == 5)
    );

    // Pages of one row walk the same rows in order.
    let mut walked = Vec::new();
    let mut after = None;
    loop {
        let page = store
            .with_conn(|conn| {
                Ok(candidates(
                    conn,
                    KERNEL,
                    5,
                    after.as_deref(),
                    NonZeroUsize::new(1).unwrap(),
                )
                .unwrap())
            })
            .unwrap();
        walked.extend(ids(&page));
        match page.last_occurrence_id {
            Some(last) if page.inspected == 1 => after = Some(last),
            _ => break,
        }
    }
    assert_eq!(walked, expected);

    // Between selection and reclamation, `a` gains a job row and `c`'s tombstone is not the one selected: both are protected; a fresh candidate for `c` under its real tombstone is reclaimed, and the shared payload survives through `b`.
    let stale = vec![
        Candidate {
            occurrence_id: occurrence_id(a),
            invalidated_commit_seq: 5,
        },
        Candidate {
            occurrence_id: occurrence_id(c),
            invalidated_commit_seq: 4,
        },
    ];
    let reclaimed = store
        .with_conn_fenced(|conn| {
            conn.execute(
                "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,attempts,created_at,updated_at)
                 VALUES ('late',?1,?2,'pending',0,1,1)",
                params![occurrence_id(a), GENERATION],
            )?;
            Ok(reclaim(conn, KERNEL, &stale, 5).unwrap())
        })
        .unwrap();
    assert_eq!(
        (reclaimed.occurrences, reclaimed.protected),
        (0, 2),
        "{reclaimed:?}"
    );
    assert_eq!(present(&store).len(), 5);
    let fresh = vec![Candidate {
        occurrence_id: occurrence_id(c),
        invalidated_commit_seq: 5,
    }];
    let reclaimed = store
        .with_conn_fenced(|conn| Ok(reclaim(conn, KERNEL, &fresh, 5).unwrap()))
        .unwrap();
    assert_eq!(
        (
            reclaimed.occurrences,
            reclaimed.payloads,
            reclaimed.protected
        ),
        (1, 1, 0)
    );
    let remaining = present(&store);
    assert!(!remaining.contains(&occurrence_id(c)));
    assert_eq!(remaining.len(), 4);
    // Once b's holder releases it, a's twin goes but the shared payload stays with b.
    let reclaimed = store
        .with_conn_fenced(|conn| {
            conn.execute(
                "DELETE FROM embedding_jobs WHERE occurrence_id=?1",
                [occurrence_id(a)],
            )?;
            Ok(reclaim(
                conn,
                KERNEL,
                &[Candidate {
                    occurrence_id: occurrence_id(a),
                    invalidated_commit_seq: 5,
                }],
                5,
            )
            .unwrap())
        })
        .unwrap();
    assert_eq!((reclaimed.occurrences, reclaimed.payloads), (1, 0));
    let payloads: i64 = store
        .with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM payloads", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(payloads, 3, "shared, live, and claim");
    assert!(present(&store).contains(&occurrence_id(live)));
    assert!(present(&store).contains(&occurrence_id(claim)));
}
