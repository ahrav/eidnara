use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group};
use kernel::Sensitivity;
use kernel::source_identity::Occurrence;
use retrieval::batch::{
    BatchBounds, MutationIdentity, ProjectionBatch, VectorGeneration, apply_batch,
    register_generation,
};
use retrieval::lexical::{AnalysisIdentity, verify_rows};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::Connection;
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

const KERNEL: &str = "kernel-bench";
const GENERATION: &str = "gen-bench";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Rows per batch, and the `max_local_mutations` bound the batch runs under.
const BATCH_ROWS: usize = 64;

fn open(dir: &std::path::Path) -> SqliteStore {
    let store = open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-bench".to_string(),
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
            install_identity(
                conn,
                &ProjectionIdentity {
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
            Ok(())
        })
        .unwrap();
    store
}

fn bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(BATCH_ROWS).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 20).unwrap(),
        max_local_mutations: NonZeroUsize::new(BATCH_ROWS).unwrap(),
        max_pending: NonZeroUsize::new(1 << 16).unwrap(),
    }
}

/// Identifier-heavy tool output: paths, symbols, error codes, and digits, so the parts column carries real work.
fn text(index: usize) -> String {
    format!(
        "src/module_{index}/HTTPServer_{index}.rs: ERR_CONN_REFUSED E{index:04} at 10.0.{}.{} \
         while getHTTPResponse2xx(sha256:{index:016x}) retried x86_64 build v1.{index}.3 \
         snake_case_identifier_{index} XMLHttpRequest fooBar{index}Baz",
        index % 256,
        (index * 7) % 256
    )
}

fn identity(index: usize) -> Vec<(String, String)> {
    vec![
        ("project_id".into(), "proj-bench".into()),
        ("harness".into(), "pi".into()),
        ("session_id".into(), "sess-bench".into()),
        ("parent_message_id".into(), "msg-bench".into()),
        ("tool_call_id".into(), format!("call-{index}")),
        ("result_revision".into(), "1".into()),
        ("block_index".into(), "0".into()),
    ]
}

fn apply_rows(store: &SqliteStore, first: usize, count: usize, commit: i64) {
    let texts: Vec<String> = (first..first + count).map(text).collect();
    let identities: Vec<Vec<(String, String)>> = (first..first + count).map(identity).collect();
    let keys: Vec<String> = (first..first + count).map(|i| format!("key-{i}")).collect();
    let borrowed: Vec<Vec<(&str, &str)>> = identities
        .iter()
        .map(|fields| {
            fields
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect()
        })
        .collect();
    let records: Vec<OccurrenceRecord<'_>> = (0..count)
        .map(|i| OccurrenceRecord {
            occurrence: Occurrence {
                class: "raw_tool_spans",
                identity: &borrowed[i],
                revision: "1",
                representation: "tool_output",
                span: None,
            },
            payload: Payload::Whole(&texts[i]),
            domain_id: "domain",
            sensitivity: Sensitivity::Normal,
            source_object_id: &keys[i],
            source_evidence_id: &keys[i],
            source_artifact_digest: DIGEST,
            created_commit_seq: commit,
        })
        .collect();
    let batch = ProjectionBatch {
        identity: MutationIdentity {
            kernel_incarnation_id: KERNEL.to_string(),
            hold_id: "hold-bench".to_string(),
            snapshot_commit_seq: 0,
            through_commit_seq: commit,
        },
        records,
        invalidations: Vec::new(),
        generation_id: Some(GENERATION),
    };
    store
        .with_conn_fenced(|conn| {
            apply_batch(conn, &batch, bounds(), commit).unwrap();
            Ok(())
        })
        .unwrap();
}

/// Bytes per FTS5 shadow table, from `dbstat`, so content duplication and index size are visible separately.
fn lexical_bytes(dir: &std::path::Path) -> Vec<(String, i64)> {
    let raw = Connection::open(dir.join("search").join("search.sqlite")).unwrap();
    raw.prepare(
        "SELECT name, sum(pgsize) FROM dbstat
         WHERE name IN ('lexical_content','lexical_data','lexical_idx','lexical_docsize','lexical_config')
         GROUP BY name ORDER BY name",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn build(store: &SqliteStore, rows: usize) {
    for (batch, first) in (0..rows).step_by(BATCH_ROWS).enumerate() {
        apply_rows(store, first, BATCH_ROWS, batch as i64 + 1);
    }
}

fn index_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("lexical_index");
    for rows in [256usize, 1024] {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        build(&store, rows);
        store
            .with_conn(|conn| {
                verify_rows(conn).unwrap();
                Ok(())
            })
            .unwrap();
        group.bench_with_input(BenchmarkId::new("verify_rows", rows), &rows, |b, _| {
            // Each iteration verifies a fresh read snapshot of the same warmed database.
            b.iter(|| {
                store
                    .with_conn(|conn| {
                        verify_rows(conn).unwrap();
                        Ok(())
                    })
                    .unwrap();
            });
        });
        drop(store);
        let payload: usize = (0..rows).map(|i| text(i).len()).sum();
        let bytes = lexical_bytes(dir.path());
        let total: i64 = bytes.iter().map(|(_, size)| size).sum();
        eprintln!(
            "observation lexical_index/{rows} rows: payload bytes {payload}, lexical bytes {total} ({bytes:?}), ratio {:.2}",
            total as f64 / payload as f64
        );
        group.bench_with_input(BenchmarkId::new("build", rows), &rows, |b, &rows| {
            b.iter_batched(
                || {
                    let dir = tempfile::tempdir().unwrap();
                    let store = open(dir.path());
                    (dir, store)
                },
                |(dir, store)| {
                    // Transactions and store close are timed; directory cleanup is not.
                    build(&store, rows);
                    black_box(dir)
                },
                BatchSize::PerIteration,
            )
        });
    }
    group.finish();
}

fn configure() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .sample_size(10)
}

criterion_group! {
    name = benches;
    config = configure();
    targets = index_benches
}

fn main() {
    if !std::env::args().any(|arg| arg == "--bench") {
        eprintln!("lexical_index: fixture setup runs only under `cargo bench`");
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
