//! The fixture fixes key population and collision depth, so each run measures
//! one parse, equality page, prefix page, and batch write at a known size.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group};
use kernel::Sensitivity;
use kernel::applicability::EvalBudget;
use kernel::source_identity::Occurrence;
use retrieval::batch::{BatchBounds, MutationIdentity, ProjectionBatch, apply_batch};
use retrieval::exact::{
    ExactQuery, HexPrefix, LookupContext, ObjectFormat, SelectorBounds, ShaPrefixQuery, classify,
    page,
};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionIdentity, install_identity};
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

const KERNEL: &str = "kernel-bench";

fn open(dir: &std::path::Path) -> SqliteStore {
    open_sqlite(
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
    .unwrap()
}

fn bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(1 << 16).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 26).unwrap(),
        max_local_mutations: NonZeroUsize::new(1 << 17).unwrap(),
        max_pending: NonZeroUsize::new(1 << 17).unwrap(),
    }
}

fn install(store: &SqliteStore) {
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
                    vector_dimension: 8,
                    generation_epoch: 1,
                },
                1,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
}

/// `commits` git commit identities followed by `claims` canonical claim identities.
fn identities(commits: usize, claims: usize) -> Vec<Vec<(String, String)>> {
    (0..commits)
        .map(|n| {
            vec![
                ("repository_id".to_string(), "repo".to_string()),
                ("object_format".to_string(), "sha1".to_string()),
                ("oid".to_string(), format!("ab{n:038x}")),
            ]
        })
        .chain((0..claims).map(|n| vec![("object_id".to_string(), format!("obj-{n:08}"))]))
        .collect()
}

fn borrow(identities: &[Vec<(String, String)>]) -> Vec<Vec<(&str, &str)>> {
    identities
        .iter()
        .map(|fields| {
            fields
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect()
        })
        .collect()
}

fn batch<'a>(borrowed: &'a [Vec<(&'a str, &'a str)>], commits: usize) -> ProjectionBatch<'a> {
    let records = borrowed
        .iter()
        .enumerate()
        .map(|(index, identity)| OccurrenceRecord {
            occurrence: Occurrence {
                class: if index < commits {
                    "git_commits"
                } else {
                    "canonical_claims"
                },
                identity,
                revision: "1",
                representation: if index < commits {
                    "commit_message"
                } else {
                    "decision_summary"
                },
                span: None,
            },
            payload: Payload::Whole("text"),
            domain_id: "domain",
            sensitivity: Sensitivity::Normal,
            source_object_id: identity[identity.len() - 1].1,
            source_evidence_id: "evidence",
            source_artifact_digest: "0000000000000000000000000000000000000000000000000000000000000000",
            created_commit_seq: 1,
        })
        .collect();
    ProjectionBatch {
        identity: MutationIdentity {
            kernel_incarnation_id: KERNEL.to_string(),
            hold_id: "hold".to_string(),
            snapshot_commit_seq: 0,
            through_commit_seq: 1,
        },
        records,
        invalidations: vec![],
        generation_id: None,
    }
}

fn apply(store: &SqliteStore, batch: &ProjectionBatch<'_>) -> usize {
    store
        .with_conn_fenced(|conn| Ok(apply_batch(conn, batch, bounds(), 1).unwrap()))
        .unwrap()
        .associations_inserted
}

fn parse_benches(c: &mut Criterion) {
    let bounds = SelectorBounds {
        max_input_bytes: NonZeroUsize::new(4096).unwrap(),
        max_value_bytes: NonZeroUsize::new(1024).unwrap(),
    };
    let mut group = c.benchmark_group("exact_parse");
    for (name, request) in [
        ("direct_id", "id:obj-00000001".to_string()),
        ("direct_sha_full", format!("sha:{}", "a".repeat(40))),
        (
            "direct_path_escaped",
            "path:src/%FF/deep/%20name.rs".to_string(),
        ),
        (
            "direct_quoted_command",
            "command:\"git log --oneline -n 20 \\u00e9\"".to_string(),
        ),
        (
            "hybrid_prose",
            "explain why symbol:Foo::bar fails with error:E0308 in path:src/lib.rs".to_string(),
        ),
        ("prose_no_selector", "x".repeat(2048)),
    ] {
        group.bench_with_input(
            BenchmarkId::new(name, request.len()),
            &request,
            |b, request| b.iter(|| black_box(classify(black_box(request), bounds))),
        );
    }
    group.finish();
}

fn page_benches(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    install(&store);
    let identities = identities(4_096, 4_096);
    let borrowed = borrow(&identities);
    apply(&store, &batch(&borrowed, 4_096));
    let budget = EvalBudget::unbounded();
    let mut group = c.benchmark_group("exact_page");
    for page_rows in [16usize, 256] {
        let context = LookupContext {
            kernel_incarnation_id: KERNEL,
            page_rows: NonZeroUsize::new(page_rows).unwrap(),
            budget: &budget,
        };
        let equality = ExactQuery::CanonicalObject(b"obj-00002048");
        group.bench_with_input(
            BenchmarkId::new("key_equality", page_rows),
            &context,
            |b, context| {
                b.iter(|| {
                    store
                        .with_conn(|conn| Ok(page(conn, context, &equality, None).unwrap()))
                        .unwrap()
                })
            },
        );
        let prefix = HexPrefix::parse("ab").unwrap();
        let sha =
            ExactQuery::Sha(ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &prefix).unwrap());
        group.bench_with_input(
            BenchmarkId::new("sha_prefix_first_page", page_rows),
            &context,
            |b, context| {
                b.iter(|| {
                    store
                        .with_conn(|conn| Ok(page(conn, context, &sha, None).unwrap()))
                        .unwrap()
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("sha_prefix_exhaust", page_rows),
            &context,
            |b, context| {
                b.iter_batched(
                    || None,
                    |mut cursor| {
                        let mut pages = 0usize;
                        loop {
                            let page = store
                                .with_conn(|conn| {
                                    Ok(page(conn, context, &sha, cursor.as_ref()).unwrap())
                                })
                                .unwrap();
                            pages += 1;
                            cursor = page.next;
                            if cursor.is_none() {
                                break;
                            }
                        }
                        black_box(pages)
                    },
                    BatchSize::SmallInput,
                )
            },
        );
    }
    group.finish();
}

/// One batch of commits and claims into a fresh projection: every record
/// derives one association, so this is the write cost the index adds.
fn write_benches(c: &mut Criterion) {
    let identities = identities(1_024, 1_024);
    let borrowed = borrow(&identities);
    let batch = batch(&borrowed, 1_024);
    let mut group = c.benchmark_group("exact_write");
    group.bench_function(BenchmarkId::new("apply_batch", batch.records.len()), |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let store = open(dir.path());
                install(&store);
                (dir, store)
            },
            |(_dir, store)| black_box(apply(&store, &batch)),
            BatchSize::PerIteration,
        )
    });
    group.finish();
}

fn configure() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20)
}

criterion_group! {
    name = benches;
    config = configure();
    targets = parse_benches, page_benches, write_benches
}

fn main() {
    if !std::env::args().any(|arg| arg == "--bench") {
        eprintln!("exact_lookup: fixture setup runs only under `cargo bench`");
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
