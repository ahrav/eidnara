//! A kernel with admitted decisions and a search projection listing them as canonical-claim occurrences, so a reader test can rank layer files against live projection rows and canonical eligibility.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CommitIntent, DecisionPayload,
    DecisionSpec, Dimension, DomainSpec, EventKind, KernelStore, ProjectScope, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, ProjectionBatch, VectorGeneration, apply_batch,
    register_generation,
};
use retrieval::eligibility::Authority;
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionIdentity, install_identity};
use sha2::{Digest, Sha256};
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

const DOMAIN: &str = "domain";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const HOLD: &str = "hold-1";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub struct Projection {
    pub kernel: Arc<KernelStore>,
    pub store: SqliteStore,
    pub incarnation: String,
    pub project: ProjectScope,
    pub objects: Vec<String>,
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "vector-reader-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "retrieval".to_string(),
    }
}

/// The occurrence identifier of a canonical-claim decision summary for `object`, as the projection derives it.
pub fn occurrence_id(object: &str) -> String {
    let identity = [("object_id", object)];
    encode_preserving_span(&Occurrence {
        class: OccurrenceClass::CanonicalClaims.code(),
        identity: &identity,
        revision: "1",
        representation: "decision_summary",
        span: None,
    })
    .unwrap()
    .occurrence_id
}

impl Projection {
    /// Seeds the kernel with a decision per object, admitting `admitted`, and projects every object as a live occurrence of `generation` with the identity `identity` names.
    pub fn new(
        root: &Path,
        identity: &ProjectionIdentity,
        generation: &VectorGeneration,
        objects: &[&str],
        admitted: &[&str],
    ) -> Self {
        let kernel = KernelStore::open(root.join("kernel")).unwrap();
        kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
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
                        object_id: (*object).to_string(),
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
                        source_kind: "repository".to_string(),
                        source_id: (*object).to_string(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                    })?;
                    if admitted.contains(object) {
                        envelope.record_admission(AdmissionRequest {
                            candidate_id: None,
                            subject_object_id: Some((*object).to_string()),
                            source_class: Some(SourceClass::ExplicitUser),
                            taint_class: Some(TaintClass::UserExplicit),
                            event: AdmissionEvent {
                                kind: EventKind::Other,
                                trigger_object_id: None,
                                approval_object_id: None,
                                evidence_id: None,
                                reason: "test".to_string(),
                            },
                        })?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
        let incarnation = kernel
            .database_incarnation_id_within_budget(&EvalBudget::unbounded())
            .unwrap();
        let store = open_sqlite(
            &StorageDescriptor {
                module_id: "eidnara-test".to_string(),
                storage_namespace: "search-projection".to_string(),
                isolation: Isolation::Module,
                backend: StorageBackend::Sqlite {
                    path: root
                        .join("search")
                        .join("search.sqlite")
                        .to_string_lossy()
                        .into_owned(),
                },
            },
            retrieval::BASELINE,
        )
        .unwrap();
        let mut projected = identity.clone();
        projected.kernel_incarnation_id = incarnation.clone();
        store
            .with_conn_fenced(|conn| {
                install_identity(conn, &projected, 1).unwrap();
                register_generation(conn, generation, 1).unwrap();
                Ok(())
            })
            .unwrap();
        let projection = Self {
            kernel: Arc::new(kernel),
            store,
            incarnation,
            project: ProjectScope::new(PROJECT).unwrap(),
            objects: objects.iter().map(|object| (*object).to_string()).collect(),
        };
        projection.project(&generation.generation_id);
        projection
    }

    fn project(&self, generation_id: &str) {
        let through = self.kernel.tip().unwrap();
        let texts: Vec<String> = self
            .objects
            .iter()
            .map(|object| format!("text {object}"))
            .collect();
        let identities: Vec<[(&str, &str); 1]> = self
            .objects
            .iter()
            .map(|object| [("object_id", object.as_str())])
            .collect();
        let records: Vec<OccurrenceRecord<'_>> = self
            .objects
            .iter()
            .zip(&identities)
            .zip(&texts)
            .map(|((object, identity), text)| OccurrenceRecord {
                occurrence: Occurrence {
                    class: OccurrenceClass::CanonicalClaims.code(),
                    identity,
                    revision: "1",
                    representation: "decision_summary",
                    span: None,
                },
                payload: Payload::Whole(text),
                domain_id: DOMAIN,
                sensitivity: Sensitivity::Normal,
                source_object_id: object,
                source_evidence_id: "evidence",
                source_artifact_digest: DIGEST,
                created_commit_seq: through,
            })
            .collect();
        let batch = ProjectionBatch {
            identity: MutationIdentity {
                kernel_incarnation_id: self.incarnation.clone(),
                hold_id: HOLD.to_string(),
                snapshot_commit_seq: 0,
                through_commit_seq: through,
            },
            records,
            invalidations: vec![],
            generation_id: Some(generation_id),
        };
        let bounds = BatchBounds {
            persist: PersistBounds {
                max_records: NonZeroUsize::new(4096).unwrap(),
                max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
                max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
            },
            max_source_bytes: NonZeroUsize::new(1 << 20).unwrap(),
            max_local_mutations: NonZeroUsize::new(8192).unwrap(),
            max_pending: NonZeroUsize::new(8192).unwrap(),
        };
        self.store
            .with_conn_fenced(|conn| {
                apply_batch(conn, &batch, bounds, through).unwrap();
                // Every occurrence's embedding work is complete as far as the projection knows; the vectors live in layer files.
                conn.execute("UPDATE embedding_jobs SET state='embedded'", [])?;
                Ok(())
            })
            .unwrap();
    }

    pub fn authority(&self) -> Authority<'_> {
        Authority {
            project: &self.project,
            destination: ArtifactDestination::Local,
        }
    }

    pub fn retire(&self, object: &str) {
        self.kernel
            .commit(intent(&format!("retire-{object}")), |envelope| {
                envelope.retire_decision(object)?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Marks the occurrence dead in the projection, as an invalidation would.
    pub fn tombstone(&self, object: &str) {
        self.store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at) VALUES (?1,7,'retired',1)",
                    [occurrence_id(object)],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

/// f64 inner product in coordinate order, score descending then identifier ascending: the reference a ranking is checked against.
pub fn reference(query: &[f32], rows: &[(String, Vec<f32>)]) -> Vec<(String, f64)> {
    let mut scored: Vec<(String, f64)> = rows
        .iter()
        .map(|(id, vector)| {
            let mut score = 0.0f64;
            for j in 0..vector.len() {
                score += f64::from(query[j]) * f64::from(vector[j]);
            }
            (id.clone(), score)
        })
        .collect();
    scored.sort_by(|(a_id, a), (b_id, b)| b.total_cmp(a).then_with(|| a_id.cmp(b_id)));
    scored
}
