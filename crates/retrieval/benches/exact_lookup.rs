//! The fixture fixes key population and collision depth, so each run measures
//! one parse, equality page, prefix page, batch write, and resolution at a
//! known size.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, Span};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CommitIntent, DecisionPayload,
    DecisionSpec, Dimension, DomainSpec, EventKind, KernelStore, MAX_ELIGIBILITY_CANDIDATES,
    ProjectScope, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, ProjectionBatch, apply_batch, read_checkpoint,
};
use retrieval::exact::{
    Authority, CompletenessCertificate, ExactQuery, HexPrefix, LookupContext, ObjectFormat,
    RequestIntent, ResolveBounds, ResolveRequest, SelectorBounds, ShaPrefixQuery, classify, page,
    resolve, validate_for_use,
};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionIdentity, install_identity};
use sha2::{Digest, Sha256};
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

const KERNEL: &str = "kernel-bench";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const DOMAIN: &str = "domain";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Four full pages at 256 rows per page, and exactly the retention cap.
const FANOUT_ROWS: usize = 1024;

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

fn install(store: &SqliteStore, kernel_incarnation_id: &str) {
    store
        .with_conn_fenced(|conn| {
            install_identity(
                conn,
                &ProjectionIdentity {
                    schema_version: retrieval::SCHEMA_VERSION,
                    kernel_incarnation_id: kernel_incarnation_id.to_string(),
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
            source_artifact_digest: DIGEST,
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

fn apply(store: &SqliteStore, batch: &ProjectionBatch<'_>, through: i64) -> usize {
    store
        .with_conn_fenced(|conn| Ok(apply_batch(conn, batch, bounds(), through).unwrap()))
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
    install(&store, KERNEL);
    let identities = identities(4_096, 4_096);
    let borrowed = borrow(&identities);
    apply(&store, &batch(&borrowed, 4_096), 1);
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
                install(&store, KERNEL);
                (dir, store)
            },
            |(_dir, store)| black_box(apply(&store, &batch, 1)),
            BatchSize::PerIteration,
        )
    });
    group.finish();
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "exact-bench".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "bench".to_string(),
        cause: "measure".to_string(),
    }
}

/// `objects` admitted decisions under one project scope, all eligible.
fn kernel(root: &std::path::Path, objects: &[String]) -> KernelStore {
    let kernel = KernelStore::open(root.join("kernel")).unwrap();
    kernel
        .commit(intent("seed"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: DOMAIN.to_string(),
                object_id: "domain-object".to_string(),
                name: "bench".to_string(),
                source_kind: "bench".to_string(),
                source_id: DOMAIN.to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            envelope.insert_scope(ScopeSpec {
                scope_id: SCOPE.to_string(),
                object_id: SCOPE.to_string(),
                source_id: SCOPE.to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "kernel_route".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: vec![ScopeTermSpec {
                    dimension: Dimension::Project.as_str().to_string(),
                    operator: "exact".to_string(),
                    exact_value: Some(PROJECT.to_string()),
                    ..ScopeTermSpec::default()
                }],
            })?;
            for object in objects {
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("decision-{object}"),
                    object_id: object.clone(),
                    domain_id: DOMAIN.to_string(),
                    proposition_id: None,
                    scope_id: Some(SCOPE.to_string()),
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: "architecture".to_string(),
                    payload: DecisionPayload {
                        summary: format!("summary {object}"),
                        rationale: format!("rationale {object}"),
                    },
                    source_kind: "repo".to_string(),
                    source_id: format!("src/{object}"),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.record_admission(AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(object.clone()),
                    source_class: Some(SourceClass::ExplicitUser),
                    taint_class: Some(TaintClass::UserExplicit),
                    event: AdmissionEvent {
                        kind: EventKind::Other,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: None,
                        reason: "bench".to_string(),
                    },
                })?;
            }
            Ok(String::new())
        })
        .unwrap();
    kernel
}

fn resolve_benches(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let mut objects: Vec<String> = (0..256).map(|n| format!("obj-{n:08}")).collect();
    objects.push("obj-fanout".to_string());
    let kernel = kernel(dir.path(), &objects);
    let budget = EvalBudget::unbounded();
    let incarnation = kernel
        .database_incarnation_id_within_budget(&budget)
        .unwrap();
    let through = kernel.tip().unwrap();
    let store = open(dir.path());
    install(&store, &incarnation);
    let identities = identities(0, 256);
    let borrowed = borrow(&identities);
    // `FANOUT_ROWS` spans of one object share the key `obj-fanout`, so a single
    // resolve enumerates them across several pages.
    let fanout_identity = [("object_id", "obj-fanout")];
    let fanout_text = "x".repeat(FANOUT_ROWS + 1);
    let mut batch = batch(&borrowed, 0);
    batch.identity.kernel_incarnation_id = incarnation.clone();
    batch.identity.through_commit_seq = through;
    batch
        .records
        .extend((0..FANOUT_ROWS).map(|n| OccurrenceRecord {
            occurrence: Occurrence {
                class: "canonical_claims",
                identity: &fanout_identity,
                revision: "1",
                representation: "decision_summary",
                span: Some(Span {
                    start: n as u64,
                    end: n as u64 + 1,
                }),
            },
            payload: Payload::Whole(&fanout_text),
            domain_id: "domain",
            sensitivity: Sensitivity::Normal,
            source_object_id: "obj-fanout",
            source_evidence_id: "evidence",
            source_artifact_digest: DIGEST,
            created_commit_seq: through,
        }));
    for record in &mut batch.records {
        record.created_commit_seq = through;
    }
    apply(&store, &batch, through);
    let checkpoint = store
        .with_conn(|conn| Ok(read_checkpoint(conn, &incarnation).unwrap().unwrap()))
        .unwrap();
    let certificate = CompletenessCertificate {
        canonical_incarnation_id: incarnation.clone(),
        inventory_epoch: "bench-epoch".to_string(),
        identity_contract_version: "search-projection-identity-v3".to_string(),
        extraction_version: retrieval::exact::EXTRACTION_VERSION,
        complete_through_commit_seq: checkpoint.checkpoint_commit_seq,
        projection: checkpoint,
    };
    let project = ProjectScope::new(PROJECT).unwrap();
    let authority = Authority {
        project: &project,
        destination: ArtifactDestination::Local,
        inventory_epoch: "bench-epoch",
    };
    let bounds = ResolveBounds {
        page_rows: NonZeroUsize::new(256).unwrap(),
        max_rows: NonZeroUsize::new(4_096).unwrap(),
        max_pages: NonZeroUsize::new(64).unwrap(),
        max_retained: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        max_retained_bytes: NonZeroUsize::new(1 << 22).unwrap(),
    };
    let request = ResolveRequest {
        query: ExactQuery::CanonicalObject(b"obj-00000128"),
        intent: RequestIntent::WholeRequest,
        certificate: &certificate,
        authority,
        bounds,
    };
    let mut group = c.benchmark_group("exact_resolve");
    group.bench_function("singleton_proof", |b| {
        b.iter(|| {
            let resolution = store
                .with_conn(|conn| Ok(resolve(conn, &kernel, &request, &budget).unwrap()))
                .unwrap();
            assert!(resolution.proof.is_some());
            black_box(resolution)
        })
    });
    let fanout_request = ResolveRequest {
        query: ExactQuery::CanonicalObject(b"obj-fanout"),
        intent: RequestIntent::WholeRequest,
        certificate: &certificate,
        authority,
        bounds,
    };
    group.bench_function(BenchmarkId::new("fanout_proof", FANOUT_ROWS), |b| {
        b.iter(|| {
            let resolution = store
                .with_conn(|conn| Ok(resolve(conn, &kernel, &fanout_request, &budget).unwrap()))
                .unwrap();
            assert_eq!(
                resolution.consumed.rows, FANOUT_ROWS,
                "the fanout attempt exercises the per-row and per-page paths"
            );
            assert!(resolution.proof.is_some());
            black_box(resolution)
        })
    });
    let proof = store
        .with_conn(|conn| Ok(resolve(conn, &kernel, &request, &budget).unwrap()))
        .unwrap()
        .proof
        .unwrap();
    group.bench_function("final_use_validation", |b| {
        b.iter(|| {
            let outcome = store
                .with_conn(|conn| {
                    Ok(validate_for_use(conn, &kernel, &proof, authority, &budget).unwrap())
                })
                .unwrap();
            assert_eq!(outcome, Ok(()), "the bench times the full re-judgment path");
        })
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
    targets = parse_benches, page_benches, write_benches, resolve_benches
}

fn main() {
    if !std::env::args().any(|arg| arg == "--bench") {
        eprintln!("exact_lookup: fixture setup runs only under `cargo bench`");
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
