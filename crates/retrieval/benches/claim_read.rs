#[path = "../../kernel/tests/claim_fixture/mod.rs"]
mod claim_fixture;

use std::hint::black_box;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use claim_fixture::{Fixture, admission, decision, direct, intent, request};
use criterion::{BenchmarkId, Criterion};
use kernel::applicability::EvalBudget;
use kernel::source_identity::Occurrence;
use kernel::{ClaimFactBounds, ClaimFactsError, Sensitivity};
use retrieval::claims::{
    CandidateState, ClaimCandidateBatch, ClaimCandidateBounds, ClaimCandidateError,
    classify_live_claims, live_claim_candidates,
};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionError, persist_occurrences};
use rusqlite::params;
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const REPRESENTATIONS: [(&str, &str); 3] = [
    ("canonical_claims", "decision_summary"),
    ("canonical_claims", "rationale"),
    ("promoted_memory", "summary"),
];

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

fn project(conn: &GuardedConn<'_>, record: OccurrenceRecord<'_>) -> String {
    let persisted = persist_occurrences(
        conn,
        std::slice::from_ref(&record),
        PersistBounds {
            max_records: nonzero(1),
            max_payload_bytes: nonzero(1 << 16),
            max_tuple_bytes: nonzero(2048),
        },
        1,
    )
    .unwrap()
    .remove(0);
    for key in retrieval::exact::extract(&record, &persisted.lineage_id) {
        conn.execute(
            "INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                 extraction_version,created_commit_seq) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                key.family.keyword(),
                key.namespace,
                key.key,
                persisted.occurrence_id,
                key.target_id,
                retrieval::exact::EXTRACTION_VERSION,
                record.created_commit_seq
            ],
        )
        .unwrap();
    }
    persisted.occurrence_id
}

struct Workload {
    projection: SqliteStore,
    canonical: Fixture,
    objects: Vec<String>,
    bounds: ClaimCandidateBounds,
    budget: EvalBudget,
}

impl Workload {
    fn new(objects: usize, history: usize, tombstones: usize, payload_bytes: usize) -> Self {
        let canonical = Fixture::open();
        let projection = open_sqlite(
            &StorageDescriptor {
                module_id: "eidnara-bench".to_string(),
                storage_namespace: "search-projection".to_string(),
                isolation: Isolation::Module,
                backend: StorageBackend::Sqlite {
                    path: canonical
                        .root
                        .path()
                        .join("search.sqlite")
                        .to_string_lossy()
                        .into_owned(),
                },
            },
            retrieval::BASELINE,
        )
        .unwrap();
        let text = "x".repeat(payload_bytes);
        let acquisition = canonical.retain("acquisition", "observed claim evidence");
        let mut ids = Vec::new();
        for index in 0..objects {
            let index = i64::try_from(index).unwrap();
            let object = format!("decision-object-{index}");
            canonical
                .store
                .commit(intent(&format!("decision-{index}")), |envelope| {
                    let mut spec = decision(index, 1);
                    spec.source_id = object.clone();
                    spec.payload.summary = text.clone();
                    spec.payload.rationale = text.clone();
                    envelope.insert_decision(spec)?;
                    envelope.record_admission(admission(&object))?;
                    Ok(String::new())
                })
                .unwrap();
            for step in 1..history {
                canonical
                    .store
                    .commit(intent(&format!("history-{index}-{step}")), |envelope| {
                        envelope.record_admission(admission(&object))?;
                        Ok(String::new())
                    })
                    .unwrap();
            }
            canonical
                .record(
                    &format!("causality-{index}"),
                    request(&object, 1, direct(&acquisition.0, &acquisition.1)),
                )
                .unwrap();
            for (class, representation) in REPRESENTATIONS {
                let published = canonical.publish(class, representation, index, 1, &text);
                let field = if class == "promoted_memory" {
                    "decision_object_id"
                } else {
                    "object_id"
                };
                let identity = [(field, object.as_str())];
                let occurrence_id = projection
                    .with_conn_fenced(|conn| {
                        Ok(project(
                            conn,
                            OccurrenceRecord {
                                occurrence: Occurrence {
                                    class,
                                    identity: &identity,
                                    revision: "1",
                                    representation,
                                    span: None,
                                },
                                payload: Payload::Whole(&text),
                                domain_id: claim_fixture::DOMAIN,
                                sensitivity: Sensitivity::Normal,
                                source_object_id: &published.descriptor_object_id,
                                source_evidence_id: &published.evidence_id,
                                source_artifact_digest: &published.digest,
                                created_commit_seq: published.commit_seq,
                            },
                        ))
                    })
                    .unwrap();
                assert_eq!(occurrence_id, published.occurrence_id);
            }
            ids.push(object);
        }
        projection.with_conn_fenced(|conn| {
            for index in 0..tombstones {
                let object = format!("retired-{index:08}");
                let identity = [("object_id", object.as_str())];
                let occurrence_id = project(conn, OccurrenceRecord {
                    occurrence: Occurrence { class: "canonical_claims", identity: &identity, revision: "1", representation: "decision_summary", span: None },
                    payload: Payload::Whole("retired"),
                    domain_id: claim_fixture::DOMAIN,
                    sensitivity: Sensitivity::Normal,
                    source_object_id: "retired-descriptor",
                    source_evidence_id: "retired-evidence",
                    source_artifact_digest: &acquisition.1,
                    created_commit_seq: 1,
                });
                conn.execute(
                    "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
                     VALUES (?1,2,'retired',2)", [&occurrence_id],
                )?;
            }
            Ok(())
        }).unwrap();
        let budget = EvalBudget::unbounded();
        let database = canonical
            .store
            .database_incarnation_id_within_budget(&budget)
            .unwrap();
        let tip = canonical.tip();
        projection.with_conn_fenced(|conn| {
            retrieval::install_identity(conn, &retrieval::ProjectionIdentity {
                schema_version: retrieval::SCHEMA_VERSION,
                kernel_incarnation_id: database,
                projection_policy_version: "source-policy.v1".to_string(),
                identity_contract_version: "search-projection-identity-v3".to_string(),
                limit_manifest_protocol_version: "limits.v1".to_string(),
                embedding_model: "bench".to_string(),
                tokenizer_fingerprint: "bench".to_string(),
                analysis_identity: retrieval::lexical::AnalysisIdentity::current().as_str().to_string(),
                vector_dimension: 8,
                generation_epoch: 1,
            }, 1).unwrap();
            conn.execute(
                "INSERT INTO projection_checkpoint(singleton,snapshot_commit_seq,checkpoint_commit_seq,hold_id,updated_at)
                 VALUES (1,?1,?1,'bench',0)", [tip])?;
            Ok(())
        }).unwrap();
        Self {
            projection,
            canonical,
            budget,
            objects: ids,
            bounds: ClaimCandidateBounds {
                max_rows: nonzero((objects * REPRESENTATIONS.len()).max(1)),
                facts: ClaimFactBounds {
                    max_claims: nonzero(objects.max(1)),
                    max_causal_payload_bytes: NonZeroU64::new(1 << 16).unwrap(),
                },
            },
        }
    }

    fn classify(
        &self,
        bounds: ClaimCandidateBounds,
    ) -> Result<ClaimCandidateBatch, ClaimCandidateError> {
        self.projection
            .with_conn(|conn| {
                Ok(classify_live_claims(
                    conn,
                    &self.canonical.store,
                    &self.budget,
                    bounds,
                ))
            })
            .unwrap()
    }

    fn check(&self) {
        let batch = self.classify(self.bounds).unwrap();
        assert_eq!(batch.known_as_of, self.canonical.tip());
        assert_eq!(
            batch.candidates.len(),
            self.objects.len() * REPRESENTATIONS.len()
        );
        assert_eq!(batch.claims.len(), self.objects.len());
        for candidate in &batch.candidates {
            assert_eq!(candidate.state, CandidateState::Current);
            let facts = batch.claim(candidate).unwrap();
            assert_eq!(facts.object.object_id, candidate.row.object_id);
            assert_eq!(facts.occurrences.len(), REPRESENTATIONS.len());
            assert!(matches!(
                facts.causality,
                kernel::CausalClass::DirectObservation { .. }
            ));
            assert!(
                facts
                    .occurrences
                    .iter()
                    .any(|occurrence| occurrence.occurrence_id == candidate.row.occurrence_id)
            );
        }
    }
}

fn main() {
    let timing = std::env::args().any(|arg| arg == "--bench");
    let mut criterion = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20)
        .configure_from_args();
    for (name, objects, history, tombstones, bytes) in [
        ("empty", 0, 1, 0, 256),
        ("small", 16, 1, 0, 256),
        ("large", 256, 1, 0, 256),
        ("admission_history", 16, 8, 0, 256),
        ("tombstone_history", 16, 1, 4096, 256),
        ("large_payload", 16, 1, 0, 16384),
    ] {
        let workload = Workload::new(
            if timing { objects } else { objects.min(2) },
            history,
            if timing {
                tombstones
            } else {
                tombstones.min(8)
            },
            bytes,
        );
        workload.check();
        eprintln!(
            "claim_read fixture={name} objects={} rows={} history={history} tombstones={} payload_bytes={bytes}",
            workload.objects.len(),
            workload.objects.len() * 3,
            if timing {
                tombstones
            } else {
                tombstones.min(8)
            }
        );
        let mut group = criterion.benchmark_group("claim_read");
        group.bench_function(BenchmarkId::new("projection", name), |b| {
            b.iter(|| {
                black_box(
                    workload
                        .projection
                        .with_conn(|conn| {
                            Ok(live_claim_candidates(conn, workload.bounds.max_rows).unwrap())
                        })
                        .unwrap(),
                )
            })
        });
        let tip = workload.canonical.tip();
        group.bench_function(BenchmarkId::new("facts", name), |b| {
            b.iter(|| {
                black_box(
                    workload
                        .canonical
                        .store
                        .claim_facts_as_of(&workload.objects, tip, workload.bounds.facts)
                        .unwrap(),
                )
            })
        });
        group.bench_function(BenchmarkId::new("classify", name), |b| {
            b.iter(|| {
                let batch = workload.classify(workload.bounds).unwrap();
                assert_eq!(batch.candidates.len(), workload.objects.len() * 3);
                assert_eq!(batch.claims.len(), workload.objects.len());
                black_box(batch)
            })
        });
        if name == "large" {
            let row_bound = ClaimCandidateBounds {
                max_rows: nonzero(1),
                ..workload.bounds
            };
            let object_bound = ClaimCandidateBounds {
                facts: ClaimFactBounds {
                    max_claims: nonzero(1),
                    ..workload.bounds.facts
                },
                ..workload.bounds
            };
            group.bench_function("row_overflow", |b| {
                b.iter(|| {
                    assert!(matches!(
                        black_box(workload.classify(row_bound)),
                        Err(ClaimCandidateError::Projection(
                            ProjectionError::TooManyRecords { count: 2 }
                        ))
                    ));
                })
            });
            group.bench_function("object_overflow", |b| {
                b.iter(|| {
                    assert!(matches!(
                        black_box(workload.classify(object_bound)),
                        Err(ClaimCandidateError::Facts(ClaimFactsError::TooManyClaims))
                    ));
                })
            });
        }
        group.finish();
    }
    criterion.final_summary();
}
