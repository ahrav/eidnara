//! Selection cost scales with reclaimable identities. Each eligible identity appears once; ordering and the limit apply to the combined eligible set.

use std::num::{NonZeroU32, NonZeroUsize};
use std::path::Path;

use retrieval::batch::{VectorGeneration, register_generation};
use retrieval::dispatch::{EpisodeGrant, Recovery, authorize_recovery};
use retrieval::identity_sweep::{Candidate, candidates, candidates_sql, reclaim};
use rusqlite::StatementStatus;
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const CURRENT: &str = "gen-1";
const RETIRED: &str = "gen-0";

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

fn generation(id: &str, epoch: u64) -> VectorGeneration {
    VectorGeneration {
        generation_id: id.to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 2,
        generation_epoch: epoch,
    }
}

fn install_generations(conn: &GuardedConn<'_>) -> rusqlite::Result<()> {
    register_generation(conn, &generation(CURRENT, 1), 1).unwrap();
    register_generation(conn, &generation(RETIRED, 0), 1).unwrap();
    conn.execute(
        "UPDATE vector_generations SET state='retired',updated_at=2 WHERE generation_id=?1",
        [RETIRED],
    )?;
    conn.execute(
        "INSERT INTO retirement_receipts(receipt_id,generation_id,reason,operator_id,retired_at,recorded_at)
         VALUES ('receipt-gen-0',?1,'superseded','operator',2,2)",
        [RETIRED],
    )?;
    Ok(())
}

fn embedded(
    conn: &GuardedConn<'_>,
    occurrence: &str,
    generation: &str,
    job_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO payloads(payload_id,bytes,byte_length,created_at) VALUES ('payload',X'00',1,1)",
        [],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,payload_id,domain_id,sensitivity,
                                           source_object_id,source_evidence_id,source_artifact_digest,created_commit_seq,persisted_at)
         VALUES (?1,X'00',?1,'messages',1,'text','payload','domain','normal',?1,?1,?1,1,1)",
        [occurrence],
    )?;
    conn.execute(
        "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,attempts,created_at,updated_at)
         VALUES (?1,?2,?3,'embedded',1,1,1)",
        [job_id, occurrence, generation],
    )?;
    conn.execute(
        "INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at)
         VALUES (?1,?2,X'0000000000000000',2,1,1,1)",
        [occurrence, generation],
    )?;
    Ok(())
}

fn tombstone(conn: &GuardedConn<'_>, occurrence: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
         VALUES (?1,2,'retired',2)",
        [occurrence],
    )?;
    Ok(())
}

/// Exactly one identity, `gone`, is eligible; the `referenced` others are kept.
fn seed(store: &SqliteStore, referenced: usize) {
    store
        .with_conn_fenced(|conn| {
            install_generations(conn)?;
            for index in 0..referenced {
                let occurrence = format!("live-{index:06}");
                embedded(conn, &occurrence, CURRENT, &format!("job-{occurrence}"))?;
            }
            embedded(conn, "gone", CURRENT, "job-gone")?;
            tombstone(conn, "gone")
        })
        .unwrap();
}

/// VDBE steps are a deterministic cost measure, unlike wall-clock time.
fn steps(store: &SqliteStore, limit: usize) -> i32 {
    store
        .with_conn(|conn| {
            let mut statement = conn.prepare(&candidates_sql())?;
            let found: Vec<String> = statement
                .query_map(rusqlite::params![limit as i64, None::<String>], |row| {
                    row.get(0)
                })?
                .collect::<rusqlite::Result<_>>()?;
            assert_eq!(found, vec!["gone".to_string()]);
            Ok(statement.get_status(StatementStatus::VmStep))
        })
        .unwrap()
}

fn pairs(found: &[Candidate]) -> Vec<(String, String)> {
    found
        .iter()
        .map(|candidate| {
            (
                candidate.occurrence_id.clone(),
                candidate.generation_id.clone(),
            )
        })
        .collect()
}

#[test]
fn selection_cost_does_not_grow_with_referenced_finished_jobs() {
    let small = tempfile::tempdir().unwrap();
    let large = tempfile::tempdir().unwrap();
    let small_store = open(small.path());
    let large_store = open(large.path());
    seed(&small_store, 500);
    seed(&large_store, 2_000);

    let small_steps = steps(&small_store, 1);
    let large_steps = steps(&large_store, 1);
    assert!(
        large_steps <= small_steps * 2,
        "four times the referenced jobs cost {large_steps} steps against {small_steps}: selection walks the job table"
    );
}

#[test]
fn an_identity_eligible_on_both_counts_is_named_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    store
        .with_conn_fenced(|conn| {
            install_generations(conn)?;
            embedded(conn, "twice", RETIRED, "job-twice")?;
            tombstone(conn, "twice")
        })
        .unwrap();
    let found = store
        .with_conn(|conn| Ok(candidates(conn, NonZeroUsize::new(10).unwrap(), None).unwrap()))
        .unwrap();
    assert_eq!(
        pairs(&found),
        vec![("twice".to_string(), RETIRED.to_string())]
    );
}

#[test]
fn the_limit_and_order_span_both_eligibility_sources() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    store
        .with_conn_fenced(|conn| {
            install_generations(conn)?;
            embedded(conn, "retired-b", RETIRED, "job-2")?;
            embedded(conn, "retired-d", RETIRED, "job-4")?;
            embedded(conn, "gone-a", CURRENT, "job-1")?;
            embedded(conn, "gone-c", CURRENT, "job-3")?;
            tombstone(conn, "gone-a")?;
            tombstone(conn, "gone-c")
        })
        .unwrap();
    let found = store
        .with_conn(|conn| Ok(candidates(conn, NonZeroUsize::new(3).unwrap(), None).unwrap()))
        .unwrap();
    assert_eq!(
        pairs(&found),
        vec![
            ("gone-a".to_string(), CURRENT.to_string()),
            ("retired-b".to_string(), RETIRED.to_string()),
            ("gone-c".to_string(), CURRENT.to_string()),
        ]
    );
    assert!(found.iter().all(|candidate| candidate.has_vector));
}

#[test]
fn reclaim_removes_a_recovered_jobs_consumed_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let reclaimed = store
        .with_conn_fenced(|conn| {
            install_generations(conn)?;
            embedded(conn, "recovered", CURRENT, "job-recovered")?;
            conn.execute(
                "UPDATE embedding_jobs SET state='failed' WHERE job_id='job-recovered'",
                [],
            )?;
            assert!(matches!(
                authorize_recovery(
                    conn,
                    "job-recovered",
                    "operator-1",
                    EpisodeGrant {
                        allowance: NonZeroU32::new(1).unwrap(),
                        deadline: 100,
                    },
                    2,
                )
                .unwrap(),
                Recovery::Granted { .. }
            ));
            conn.execute(
                "UPDATE embedding_jobs SET state='embedded',updated_at=3 WHERE job_id='job-recovered'",
                [],
            )?;
            tombstone(conn, "recovered")?;

            let selected = candidates(conn, NonZeroUsize::new(1).unwrap(), None).unwrap();
            assert_eq!(
                pairs(&selected),
                vec![("recovered".to_string(), CURRENT.to_string())]
            );
            let result = reclaim(conn, &selected);
            assert!(
                result.is_ok(),
                "a consumed recovery authorization must not block reclamation: {result:?}"
            );
            let remaining_authorizations: i64 = conn.query_row(
                "SELECT COUNT(*) FROM embedding_recovery_authorizations WHERE job_id='job-recovered'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(remaining_authorizations, 0);
            Ok(result.unwrap())
        })
        .unwrap();
    assert_eq!(
        (reclaimed.jobs, reclaimed.vectors, reclaimed.survivors),
        (1, 1, 0)
    );
}
