//! A real kernel judges every projected row the resolver reads.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::Occurrence;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, BackupRequest, CommitIntent,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, EligibilityVerdict, EventKind,
    KernelStore, ProjectScope, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use retrieval::batch::{
    BatchBounds, Invalidation, MutationIdentity, ProjectionBatch, apply_batch, read_checkpoint,
};
use retrieval::exact::{
    Authority, CertificateRefusal, CompletenessCertificate, Completion, Disqualification,
    ExactProof, ExactQuery, HexPrefix, IncompleteReason, ObjectFormat, ProofInvalidation,
    RequestIntent, Resolution, ResolveBounds, ResolveRefusal, ResolveRequest, ShaPrefixQuery,
    resolve, validate_for_use, validate_for_use_with_hook_for_test,
};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity, Tombstone,
    TombstoneReason, install_identity,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

const DOMAIN: &str = "domain";
const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROJECT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SCOPE_A: &str = "project:a";
const SCOPE_B: &str = "project:b";
const HOLD: &str = "hold-1";
const CONTRACT: &str = "search-projection-identity-v3";
const EPOCH: &str = "inventory-epoch-1";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "exact-resolve-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn scope(scope_id: &str, digest: &str) -> ScopeSpec {
    ScopeSpec {
        scope_id: scope_id.to_string(),
        object_id: scope_id.to_string(),
        source_id: scope_id.to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "kernel_route".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms: vec![ScopeTermSpec {
            dimension: Dimension::Project.as_str().to_string(),
            operator: "exact".to_string(),
            exact_value: Some(digest.to_string()),
            ..ScopeTermSpec::default()
        }],
    }
}

fn decision(object: &str, scope_id: Option<&str>, sensitivity: Sensitivity) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{object}"),
        object_id: object.to_string(),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: scope_id.map(str::to_string),
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
        sensitivity,
    }
}

fn admission(object: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object.to_string()),
        source_class: Some(SourceClass::ExplicitUser),
        taint_class: Some(TaintClass::UserExplicit),
        event: AdmissionEvent {
            kind: EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "test".to_string(),
        },
    }
}

type Decision = (&'static str, Option<&'static str>, Sensitivity, bool);

fn ok(object: &'static str) -> Decision {
    (object, Some(SCOPE_A), Sensitivity::Normal, true)
}

struct Fixture {
    _root: tempfile::TempDir,
    kernel: KernelStore,
    store: SqliteStore,
    incarnation: String,
    project: ProjectScope,
}

fn open_store(dir: &Path) -> SqliteStore {
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

fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(256).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 20).unwrap(),
        max_local_mutations: NonZeroUsize::new(512).unwrap(),
        max_pending: NonZeroUsize::new(512).unwrap(),
    }
}

fn bounds() -> ResolveBounds {
    ResolveBounds {
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_pages: NonZeroUsize::new(64).unwrap(),
        max_retained: NonZeroUsize::new(64).unwrap(),
        max_retained_bytes: NonZeroUsize::new(1 << 16).unwrap(),
    }
}

#[derive(Debug, Clone)]
enum Row {
    Claim {
        key: &'static str,
        object: &'static str,
        revision: i64,
        representation: &'static str,
        payload: &'static str,
    },
    Commit {
        object: &'static str,
        oid: String,
    },
}

impl Row {
    fn class(&self) -> &'static str {
        match self {
            Self::Claim { .. } => "canonical_claims",
            Self::Commit { .. } => "git_commits",
        }
    }

    fn identity(&self) -> Vec<(String, String)> {
        match self {
            Self::Claim { key, .. } => vec![("object_id".into(), (*key).into())],
            Self::Commit { oid, .. } => vec![
                ("repository_id".into(), "repo".into()),
                ("object_format".into(), "sha1".into()),
                ("oid".into(), oid.clone()),
            ],
        }
    }

    fn revision(&self) -> i64 {
        match self {
            Self::Claim { revision, .. } => *revision,
            Self::Commit { .. } => 1,
        }
    }

    fn representation(&self) -> &'static str {
        match self {
            Self::Claim { representation, .. } => representation,
            Self::Commit { .. } => "commit_message",
        }
    }

    fn object(&self) -> &'static str {
        match self {
            Self::Claim { object, .. } | Self::Commit { object, .. } => object,
        }
    }

    fn occurrence_id(&self) -> String {
        let identity = self.identity();
        let borrowed: Vec<(&str, &str)> = identity
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        kernel::source_identity::encode_preserving_span(&Occurrence {
            class: self.class(),
            identity: &borrowed,
            revision: &self.revision().to_string(),
            representation: self.representation(),
            span: None,
        })
        .unwrap()
        .occurrence_id
    }
}

fn claim(object: &'static str, revision: i64, representation: &'static str) -> Row {
    Row::Claim {
        key: object,
        object,
        revision,
        representation,
        payload: object,
    }
}

fn alias(key: &'static str, object: &'static str, representation: &'static str) -> Row {
    Row::Claim {
        key,
        object,
        revision: 1,
        representation,
        payload: object,
    }
}

fn oid(prefix: &str, fill: char) -> String {
    format!("{prefix}{}", fill.to_string().repeat(40 - prefix.len()))
}

fn intent_of(whole_request: bool) -> RequestIntent {
    if whole_request {
        RequestIntent::WholeRequest
    } else {
        RequestIntent::Mention
    }
}

fn object_query(object: &str) -> ExactQuery<'_> {
    ExactQuery::CanonicalObject(object.as_bytes())
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(root.path().join("kernel")).unwrap();
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
                envelope.insert_scope(scope(SCOPE_A, PROJECT_A))?;
                envelope.insert_scope(scope(SCOPE_B, PROJECT_B))?;
                Ok(String::new())
            })
            .unwrap();
        let incarnation = kernel
            .database_incarnation_id_within_budget(&EvalBudget::unbounded())
            .unwrap();
        let store = open_store(root.path());
        store
            .with_conn_fenced(|conn| {
                install_identity(
                    conn,
                    &ProjectionIdentity {
                        schema_version: retrieval::SCHEMA_VERSION,
                        kernel_incarnation_id: incarnation.clone(),
                        projection_policy_version: "source-policy.v1".to_string(),
                        identity_contract_version: CONTRACT.to_string(),
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
        Self {
            _root: root,
            kernel,
            store,
            incarnation,
            project: ProjectScope::new(PROJECT_A).unwrap(),
        }
    }

    fn decide(&self, key: &str, objects: &[Decision]) {
        self.kernel
            .commit(intent(key), |envelope| {
                for (object, scope_id, sensitivity, admitted) in objects {
                    envelope.insert_decision(decision(object, *scope_id, *sensitivity))?;
                    if *admitted {
                        envelope.record_admission(admission(object))?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
    }

    fn retire(&self, key: &str, object: &str) {
        self.kernel
            .commit(intent(key), |envelope| {
                envelope.retire_decision(object)?;
                Ok(String::new())
            })
            .unwrap();
    }

    fn tip(&self) -> i64 {
        self.kernel.tip().unwrap()
    }

    fn project(&self, rows: &[Row], invalidations: Vec<Invalidation>) {
        let through = self.tip();
        let identities: Vec<Vec<(String, String)>> = rows.iter().map(Row::identity).collect();
        let revisions: Vec<String> = rows.iter().map(|row| row.revision().to_string()).collect();
        let texts: Vec<String> = rows
            .iter()
            .map(|row| match row {
                Row::Claim { payload, .. } => (*payload).to_string(),
                Row::Commit { oid, .. } => oid.clone(),
            })
            .collect();
        let borrowed: Vec<Vec<(&str, &str)>> = identities
            .iter()
            .map(|f| f.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect())
            .collect();
        let records: Vec<OccurrenceRecord<'_>> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| OccurrenceRecord {
                occurrence: Occurrence {
                    class: row.class(),
                    identity: &borrowed[index],
                    revision: &revisions[index],
                    representation: row.representation(),
                    span: None,
                },
                payload: Payload::Whole(&texts[index]),
                domain_id: DOMAIN,
                sensitivity: Sensitivity::Normal,
                source_object_id: row.object(),
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
            invalidations,
            generation_id: None,
        };
        self.store
            .with_conn_fenced(|conn| {
                apply_batch(conn, &batch, batch_bounds(), through).unwrap();
                Ok(())
            })
            .unwrap();
    }

    fn certificate(&self) -> CompletenessCertificate {
        let checkpoint = self
            .store
            .with_conn(|conn| Ok(read_checkpoint(conn, &self.incarnation).unwrap().unwrap()))
            .unwrap();
        CompletenessCertificate {
            canonical_incarnation_id: self.incarnation.clone(),
            inventory_epoch: EPOCH.to_string(),
            identity_contract_version: CONTRACT.to_string(),
            extraction_version: retrieval::exact::EXTRACTION_VERSION,
            complete_through_commit_seq: checkpoint.checkpoint_commit_seq,
            projection: checkpoint,
        }
    }

    fn resolve_with(
        &self,
        request: ResolveRequest<'_>,
        budget: &EvalBudget,
    ) -> Result<Resolution, ResolveRefusal> {
        self.store
            .with_conn(|conn| Ok(resolve(conn, &self.kernel, &request, budget)))
            .unwrap()
    }

    fn resolve(
        &self,
        query: ExactQuery<'_>,
        whole_request: bool,
        certificate: &CompletenessCertificate,
        bounds: ResolveBounds,
        budget: &EvalBudget,
    ) -> Result<Resolution, ResolveRefusal> {
        self.resolve_with(
            ResolveRequest {
                query,
                intent: intent_of(whole_request),
                certificate,
                authority: Authority {
                    project: &self.project,
                    destination: ArtifactDestination::Local,
                    inventory_epoch: EPOCH,
                },
                bounds,
            },
            budget,
        )
    }

    fn validate_with(
        &self,
        proof: &ExactProof,
        project: &ProjectScope,
        destination: ArtifactDestination,
        budget: &EvalBudget,
    ) -> Result<(), ProofInvalidation> {
        let authority = Authority {
            project,
            destination,
            inventory_epoch: EPOCH,
        };
        self.store
            .with_conn(|conn| {
                Ok(validate_for_use(
                    conn,
                    &self.kernel,
                    proof,
                    authority,
                    budget,
                ))
            })
            .unwrap()
            .unwrap()
    }

    fn validate(&self, proof: &ExactProof, budget: &EvalBudget) -> Result<(), ProofInvalidation> {
        self.validate_with(proof, &self.project, ArtifactDestination::Local, budget)
    }
}

#[test]
fn a_healthy_explicit_singleton_proves_and_validates_while_an_always_refuse_control_cannot() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1"), ok("obj-2")]);
    fixture.project(
        &[
            claim("obj-1", 1, "decision_summary"),
            claim("obj-1", 1, "rationale"),
            claim("obj-2", 1, "decision_summary"),
        ],
        vec![],
    );
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();
    let resolution = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(resolution.completion, Completion::Complete);
    assert_eq!(resolution.disqualified, None);
    assert_eq!(
        resolution.observed_targets,
        BTreeSet::from(["obj-1".to_string()])
    );
    assert_eq!(
        resolution.retained.len(),
        2,
        "both representations are retained"
    );
    assert_eq!(resolution.observations.eligible, 2);
    assert_eq!(resolution.consumed.validated, 2);
    let proof = resolution
        .proof
        .clone()
        .expect("a healthy singleton proves");
    assert_eq!(proof.target_id(), "obj-1");
    assert_eq!(proof.occurrences().len(), 2);
    assert!(
        std::ptr::eq(proof.occurrences(), &resolution.retained[..]),
        "the proof shares the retained rows instead of copying them past the retention bound"
    );
    assert_eq!(fixture.validate(&proof, &budget), Ok(()));

    let hybrid = fixture
        .resolve(
            object_query("obj-1"),
            false,
            &certificate,
            bounds(),
            &budget,
        )
        .unwrap();
    assert_eq!(hybrid.completion, Completion::Complete);
    assert!(
        hybrid.proof.is_none(),
        "prose around a selector never proves"
    );
    assert_eq!(
        hybrid.retained.len(),
        2,
        "hybrid keeps the validated evidence"
    );

    let absent = fixture
        .resolve(object_query("obj-9"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(absent.completion, Completion::NoMatch);
    assert!(absent.proof.is_none());
    assert!(absent.retained.is_empty());

    let foreign = ProjectScope::new(PROJECT_B).unwrap();
    let refused = fixture
        .resolve_with(
            ResolveRequest {
                query: object_query("obj-1"),
                intent: RequestIntent::WholeRequest,
                certificate: &certificate,
                authority: Authority {
                    project: &foreign,
                    destination: ArtifactDestination::Local,
                    inventory_epoch: EPOCH,
                },
                bounds: bounds(),
            },
            &budget,
        )
        .unwrap();
    assert_eq!(refused.completion, Completion::Complete);
    assert_eq!(
        refused.disqualified,
        Some(Disqualification::Verdict(EligibilityVerdict::WrongScope))
    );
    assert!(
        refused.proof.is_none(),
        "an authority that refuses everything is the always-refuse control"
    );
    assert!(refused.retained.is_empty());
    assert_eq!(refused.observations.wrong_scope, 2);
}

#[test]
fn eligible_targets_after_many_rejected_rows_are_retained_without_renewing_bypass() {
    let fixture = Fixture::new();
    fixture.decide(
        "objects",
        &[
            ok("keep"),
            ok("gone"),
            ("hidden", Some(SCOPE_A), Sensitivity::Normal, false),
            ("secret", Some(SCOPE_A), Sensitivity::Secret, true),
            ("elsewhere", Some(SCOPE_B), Sensitivity::Normal, true),
        ],
    );
    fixture.retire("retire", "gone");
    fixture.project(
        &[
            alias("shared", "gone", "decision_summary"),
            alias("shared", "hidden", "rationale"),
            claim("secret", 1, "decision_summary"),
            claim("elsewhere", 1, "decision_summary"),
            claim("keep", 1, "decision_summary"),
            claim("lonely", 1, "decision_summary"),
        ],
        vec![],
    );
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();

    let shared = fixture
        .resolve(
            object_query("shared"),
            true,
            &certificate,
            bounds(),
            &budget,
        )
        .unwrap();
    assert_eq!(shared.completion, Completion::Complete);
    assert_eq!(shared.consumed.rows, 2);
    assert_eq!(shared.observations.retracted, 1);
    assert_eq!(shared.observations.hidden, 1);
    assert!(shared.retained.is_empty());
    assert!(
        shared.proof.is_none(),
        "all-rejected is not NoMatch and not unique"
    );
    assert_ne!(shared.completion, Completion::NoMatch);

    for (object, expected) in [
        ("secret", EligibilityVerdict::ProviderSensitive),
        ("elsewhere", EligibilityVerdict::WrongScope),
    ] {
        let resolution = fixture
            .resolve(object_query(object), true, &certificate, bounds(), &budget)
            .unwrap();
        assert_eq!(
            resolution.disqualified,
            Some(Disqualification::Verdict(expected)),
            "{object}"
        );
        assert!(resolution.proof.is_none(), "{object}");
        assert!(resolution.retained.is_empty(), "{object}");
    }
    let lonely = fixture
        .resolve(
            object_query("lonely"),
            true,
            &certificate,
            bounds(),
            &budget,
        )
        .unwrap();
    assert_eq!(
        lonely.disqualified,
        Some(Disqualification::Verdict(EligibilityVerdict::Retracted)),
        "a projected row with no canonical object is retracted, not proven"
    );
    let kept = fixture
        .resolve(object_query("keep"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert!(kept.proof.is_some());

    let many = Fixture::new();
    many.decide("live", &[ok("late")]);
    let mut rejected: Vec<Row> = (0..7)
        .map(|index| Row::Claim {
            key: "crowd",
            object: ["r0", "r1", "r2", "r3", "r4", "r5", "r6"][index],
            revision: index as i64 + 1,
            representation: "decision_summary",
            payload: "crowd",
        })
        .collect();
    rejected.push(alias("crowd", "late", "rationale"));
    many.project(&rejected, vec![]);
    let certificate = many.certificate();
    let crowd = many
        .resolve(object_query("crowd"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(crowd.completion, Completion::Complete);
    assert_eq!(crowd.consumed.pages, 4);
    assert_eq!(crowd.consumed.rows, 8);
    assert_eq!(crowd.retained.len(), 1);
    assert_eq!(crowd.retained[0].source_object_id, "late");
    assert_eq!(
        crowd.observed_targets,
        BTreeSet::from(["crowd".to_string()])
    );
    assert!(matches!(
        crowd.disqualified,
        Some(Disqualification::Verdict(EligibilityVerdict::Retracted))
    ));
    assert!(
        crowd.proof.is_none(),
        "a later eligible singleton does not renew bypass within the attempt"
    );

    let superseded = Fixture::new();
    superseded.decide("objects", &[ok("obj-1")]);
    superseded.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let old = claim("obj-1", 1, "decision_summary").occurrence_id();
    superseded.decide("bump", &[]);
    superseded.project(
        &[claim("obj-1", 2, "decision_summary")],
        vec![Invalidation {
            occurrence_id: old.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: superseded.tip(),
                reason: TombstoneReason::Superseded,
            },
        }],
    );
    let certificate = superseded.certificate();
    let resolution = superseded
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(resolution.completion, Completion::Complete);
    assert_eq!(
        resolution.disqualified,
        Some(Disqualification::Tombstoned(TombstoneReason::Superseded))
    );
    assert!(
        resolution.proof.is_none(),
        "a tombstoned alias bars bypass for the attempt"
    );
    assert_eq!(resolution.observations.tombstoned, 1);
    assert_eq!(
        resolution.observations.stale, 1,
        "the kernel still holds revision one, so revision two is stale"
    );
    assert!(resolution.retained.is_empty());

    let revisions = Fixture::new();
    revisions.decide("objects", &[ok("dup")]);
    revisions.project(
        &[
            claim("dup", 1, "decision_summary"),
            claim("dup", 2, "decision_summary"),
        ],
        vec![],
    );
    let certificate = revisions.certificate();
    let resolution = revisions
        .resolve(object_query("dup"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(resolution.observations.eligible, 1);
    assert_eq!(resolution.observations.stale, 1);
    assert_eq!(resolution.retained.len(), 1);
    assert_eq!(resolution.retained[0].revision, 1);
    assert_eq!(
        resolution.disqualified,
        Some(Disqualification::Verdict(EligibilityVerdict::Stale)),
        "a live row at another revision cannot borrow the current one's authorization"
    );
    assert!(resolution.proof.is_none());
}

#[test]
fn bounds_before_exhaustion_yield_incomplete_and_never_uniqueness() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
    fixture.project(
        &[
            claim("obj-1", 1, "decision_summary"),
            claim("obj-1", 1, "rationale"),
        ],
        vec![],
    );
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();
    let one_row = ResolveBounds {
        page_rows: NonZeroUsize::new(1).unwrap(),
        max_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let resolution = fixture
        .resolve(object_query("obj-1"), true, &certificate, one_row, &budget)
        .unwrap();
    assert_eq!(
        resolution.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );
    assert!(
        resolution.proof.is_none(),
        "a singleton under a cap is not unique"
    );
    assert_eq!(
        resolution.retained.len(),
        1,
        "progress before the cap is kept"
    );

    let one_page = ResolveBounds {
        page_rows: NonZeroUsize::new(1).unwrap(),
        max_pages: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let resolution = fixture
        .resolve(object_query("obj-1"), true, &certificate, one_page, &budget)
        .unwrap();
    assert_eq!(
        resolution.completion,
        Completion::Incomplete(IncompleteReason::PageBound)
    );
    assert!(resolution.proof.is_none());

    let one_retained = ResolveBounds {
        page_rows: NonZeroUsize::new(1).unwrap(),
        max_retained: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let resolution = fixture
        .resolve(
            object_query("obj-1"),
            true,
            &certificate,
            one_retained,
            &budget,
        )
        .unwrap();
    assert_eq!(
        resolution.completion,
        Completion::Incomplete(IncompleteReason::RetentionExhausted)
    );
    assert_eq!(
        resolution.retained.len(),
        1,
        "stopped before the unretainable row"
    );
    assert_eq!(resolution.consumed.retained, 1);
    assert!(resolution.proof.is_none());

    let few_bytes = ResolveBounds {
        max_retained_bytes: NonZeroUsize::new(8).unwrap(),
        ..bounds()
    };
    let resolution = fixture
        .resolve(
            object_query("obj-1"),
            true,
            &certificate,
            few_bytes,
            &budget,
        )
        .unwrap();
    assert_eq!(
        resolution.completion,
        Completion::Incomplete(IncompleteReason::RetentionExhausted)
    );
    assert!(resolution.retained.is_empty());
    assert_eq!(resolution.observations.eligible, 0);
    assert_eq!(resolution.consumed.retained_bytes, 0);

    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(
        fixture
            .resolve(
                object_query("obj-1"),
                true,
                &certificate,
                bounds(),
                &cancelled
            )
            .unwrap_err(),
        ResolveRefusal::BudgetExhausted
    );
    let expired = EvalBudget::new(
        Some(Instant::now() + Duration::from_millis(1)),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(
        fixture
            .resolve(
                object_query("obj-1"),
                true,
                &certificate,
                bounds(),
                &expired
            )
            .unwrap_err(),
        ResolveRefusal::BudgetExhausted
    );
    let over = ResolveBounds {
        page_rows: NonZeroUsize::new(kernel::MAX_ELIGIBILITY_CANDIDATES + 1).unwrap(),
        ..bounds()
    };
    assert_eq!(
        fixture
            .resolve(object_query("obj-1"), true, &certificate, over, &budget)
            .unwrap_err(),
        ResolveRefusal::PageOverBound
    );
    let over_retained = ResolveBounds {
        max_retained: NonZeroUsize::new(kernel::MAX_ELIGIBILITY_CANDIDATES + 1).unwrap(),
        ..bounds()
    };
    assert_eq!(
        fixture
            .resolve(
                object_query("obj-1"),
                true,
                &certificate,
                over_retained,
                &budget
            )
            .unwrap_err(),
        ResolveRefusal::RetainedOverBound,
        "final use re-judges every proof occurrence in one kernel batch, \
         so retention past the batch bound could mint an unvalidatable proof"
    );
}

#[test]
fn sha_prefix_proof_requires_one_complete_oid_and_distinct_targets_stay_ambiguous() {
    let fixture = Fixture::new();
    fixture.decide(
        "objects",
        &[ok("c1"), ok("c2"), ok("c3"), ok("twin-a"), ok("twin-b")],
    );
    fixture.project(
        &[
            Row::Commit {
                object: "c1",
                oid: oid("abc1", '0'),
            },
            Row::Commit {
                object: "c2",
                oid: oid("abc2", '0'),
            },
            Row::Commit {
                object: "c3",
                oid: oid("abc3", '0'),
            },
            Row::Claim {
                key: "twin-a",
                object: "twin-a",
                revision: 1,
                representation: "decision_summary",
                payload: "same words",
            },
            Row::Claim {
                key: "twin-b",
                object: "twin-b",
                revision: 1,
                representation: "decision_summary",
                payload: "same words",
            },
        ],
        vec![],
    );
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();
    let sha = |prefix: &str| {
        let prefix = HexPrefix::parse(prefix).unwrap();
        let query = ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &prefix).unwrap();
        fixture
            .resolve(
                ExactQuery::Sha(query),
                true,
                &certificate,
                bounds(),
                &budget,
            )
            .unwrap()
    };
    let colliding = sha("abc");
    assert_eq!(colliding.completion, Completion::Complete);
    assert_eq!(colliding.distinct_keys, 3);
    assert_eq!(
        colliding.consumed.rows, 3,
        "enumeration does not stop at the second target"
    );
    assert_eq!(
        colliding.observed_targets.len(),
        3,
        "several eligible commits are ambiguous"
    );
    assert!(colliding.proof.is_none());
    assert_eq!(colliding.retained.len(), 3, "all stay as hybrid evidence");

    let unique = sha("abc1");
    assert_eq!(unique.distinct_keys, 1);
    assert_eq!(unique.observed_targets.len(), 1);
    let proof = unique.proof.expect("one complete oid proves");
    assert_eq!(proof.occurrences()[0].key, oid("abc1", '0').into_bytes());
    assert_eq!(fixture.validate(&proof, &budget), Ok(()));

    let none = sha("e");
    assert_eq!(none.completion, Completion::NoMatch);

    let twin_a = fixture
        .resolve(
            object_query("twin-a"),
            true,
            &certificate,
            bounds(),
            &budget,
        )
        .unwrap();
    let twin_b = fixture
        .resolve(
            object_query("twin-b"),
            true,
            &certificate,
            bounds(),
            &budget,
        )
        .unwrap();
    assert_eq!(
        twin_a.retained[0].payload_id, twin_b.retained[0].payload_id,
        "equal payload bytes share one payload row"
    );
    assert_ne!(
        twin_a.proof.as_ref().unwrap().target_id(),
        twin_b.proof.as_ref().unwrap().target_id(),
        "but each key proves its own canonical target"
    );
    assert_eq!(
        twin_a.observed_targets,
        BTreeSet::from(["twin-a".to_string()])
    );
}

#[test]
fn certificates_must_name_this_projection_and_kernel_and_lag_defeats_proof() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
    fixture.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();
    let refusal = |certificate: &CompletenessCertificate, epoch: &str| {
        fixture
            .resolve_with(
                ResolveRequest {
                    query: object_query("obj-1"),
                    intent: RequestIntent::WholeRequest,
                    certificate,
                    authority: Authority {
                        project: &fixture.project,
                        destination: ArtifactDestination::Local,
                        inventory_epoch: epoch,
                    },
                    bounds: bounds(),
                },
                &budget,
            )
            .unwrap_err()
    };
    let mut other_kernel = certificate.clone();
    other_kernel.canonical_incarnation_id = "someone-else".into();
    assert_eq!(
        refusal(&other_kernel, EPOCH),
        ResolveRefusal::Certificate(CertificateRefusal::IncarnationMismatch)
    );
    assert_eq!(
        refusal(&certificate, "inventory-epoch-2"),
        ResolveRefusal::Certificate(CertificateRefusal::InventoryEpochMismatch {
            certified: EPOCH.into(),
            current: "inventory-epoch-2".into(),
        })
    );
    let mut old_extraction = certificate.clone();
    old_extraction.extraction_version = 0;
    assert_eq!(
        refusal(&old_extraction, EPOCH),
        ResolveRefusal::Certificate(CertificateRefusal::ExtractionVersion {
            certified: 0,
            expected: retrieval::exact::EXTRACTION_VERSION,
        })
    );
    let mut other_contract = certificate.clone();
    other_contract.identity_contract_version = "v0".into();
    assert_eq!(
        refusal(&other_contract, EPOCH),
        ResolveRefusal::Certificate(CertificateRefusal::IdentityContract)
    );
    let mut other_projection = certificate.clone();
    other_projection.projection.hold_id = "hold-2".into();
    assert_eq!(
        refusal(&other_projection, EPOCH),
        ResolveRefusal::Certificate(CertificateRefusal::ProjectionMismatch)
    );
    let mut short = certificate.clone();
    short.complete_through_commit_seq -= 1;
    assert_eq!(
        refusal(&short, EPOCH),
        ResolveRefusal::Certificate(CertificateRefusal::CompleteThrough)
    );

    fixture.decide("collision", &[ok("obj-1-collider")]);
    let lagging = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(lagging.completion, Completion::Complete);
    assert_eq!(
        lagging.disqualified,
        Some(Disqualification::ProjectionLag {
            tip: fixture.tip(),
            complete_through: certificate.complete_through_commit_seq,
        }),
        "an unprojected collision may exist past the certified horizon"
    );
    assert!(
        lagging.proof.is_none(),
        "the known winner alone cannot prove"
    );
    assert_eq!(
        lagging.retained.len(),
        1,
        "the eligible row still reaches hybrid"
    );

    fixture.project(&[], vec![]);
    assert_eq!(
        refusal(&certificate, EPOCH),
        ResolveRefusal::Certificate(CertificateRefusal::ProjectionMismatch),
        "a certificate for an older checkpoint no longer describes the projection"
    );
    let fresh = fixture.certificate();
    let resolution = fixture
        .resolve(object_query("obj-1"), true, &fresh, bounds(), &budget)
        .unwrap();
    assert!(resolution.proof.is_some());

    let empty = Fixture::new();
    let refused = empty
        .resolve_with(
            ResolveRequest {
                query: object_query("obj-1"),
                intent: RequestIntent::WholeRequest,
                certificate: &fresh,
                authority: Authority {
                    project: &empty.project,
                    destination: ArtifactDestination::Local,
                    inventory_epoch: EPOCH,
                },
                bounds: bounds(),
            },
            &budget,
        )
        .unwrap_err();
    assert_eq!(
        refused,
        ResolveRefusal::Certificate(CertificateRefusal::IncarnationMismatch),
        "a certificate for another kernel never reaches the rows"
    );
}

/// The kernel judges the occurrence's source object; the proof names the
/// association target. A row retargeted by rewriting both its key and its
/// target, while its tuple still names the original object, must refuse the
/// attempt rather than prove the new target.
#[test]
fn an_association_retargeted_against_its_tuple_refuses_the_attempt() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1"), ok("obj-2")]);
    fixture.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let certificate = fixture.certificate();
    fixture
        .store
        .with_conn_fenced(|conn| {
            let altered = conn
                .execute(
                    "UPDATE exact_associations SET key=?2, target_id='obj-2' WHERE key=?1",
                    [b"obj-1".as_slice(), b"obj-2".as_slice()],
                )
                .unwrap();
            assert_eq!(altered, 1);
            Ok(())
        })
        .unwrap();
    let outcome = fixture.resolve(
        object_query("obj-2"),
        true,
        &certificate,
        bounds(),
        &EvalBudget::unbounded(),
    );
    assert_eq!(
        outcome,
        Err(ResolveRefusal::Projection(ProjectionError::CorruptRow))
    );
}

#[test]
fn final_use_revalidation_defeats_every_later_change() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
    fixture.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();
    let proof = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap()
        .proof
        .unwrap();
    assert_eq!(fixture.validate(&proof, &budget), Ok(()));

    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(
        fixture.validate(&proof, &cancelled),
        Err(ProofInvalidation::BudgetExhausted)
    );
    let other = ProjectScope::new(PROJECT_B).unwrap();
    assert_eq!(
        fixture.validate_with(&proof, &other, ArtifactDestination::Local, &budget),
        Err(ProofInvalidation::AuthorityMismatch)
    );
    let moved = Authority {
        project: &fixture.project,
        destination: ArtifactDestination::Local,
        inventory_epoch: "inventory-epoch-2",
    };
    assert_eq!(
        fixture
            .store
            .with_conn(|conn| Ok(validate_for_use(
                conn,
                &fixture.kernel,
                &proof,
                moved,
                &budget
            )))
            .unwrap()
            .unwrap(),
        Err(ProofInvalidation::InventoryEpochChanged)
    );
    assert_eq!(
        fixture.validate_with(
            &proof,
            &fixture.project,
            ArtifactDestination::Remote,
            &budget
        ),
        Err(ProofInvalidation::AuthorityMismatch),
        "a proof for one destination grants nothing at another"
    );

    let proven = fixture.tip();
    fixture.decide("unrelated", &[ok("obj-2")]);
    assert_eq!(
        fixture.validate(&proof, &budget),
        Err(ProofInvalidation::CanonicalChanged {
            proven,
            current: fixture.tip(),
        }),
        "any later canonical commit stales the horizon"
    );

    fixture.project(&[claim("obj-2", 1, "decision_summary")], vec![]);
    assert_eq!(
        fixture.validate(&proof, &budget),
        Err(ProofInvalidation::ProjectionChanged)
    );

    let certificate = fixture.certificate();
    let proof = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap()
        .proof
        .unwrap();
    let proven = fixture.tip();
    fixture.retire("retire", "obj-1");
    assert_eq!(
        fixture.validate(&proof, &budget),
        Err(ProofInvalidation::CanonicalChanged {
            proven,
            current: fixture.tip(),
        })
    );
    fixture.project(&[], vec![]);
    let certificate = fixture.certificate();
    let resolution = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(
        resolution.disqualified,
        Some(Disqualification::Verdict(EligibilityVerdict::Retracted))
    );
    assert!(resolution.proof.is_none());

    let rollback = Fixture::new();
    rollback.decide("objects", &[ok("obj-1")]);
    let backup_dir = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(backup_dir.path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    let manifest = rollback
        .kernel
        .backup(BackupRequest {
            destination_directory: backup_dir.path().to_path_buf(),
            deadline: Instant::now() + Duration::from_secs(10),
            capture_pin_expires_at: None,
        })
        .unwrap();
    rollback.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let certificate = rollback.certificate();
    let proof = rollback
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap()
        .proof
        .unwrap();
    rollback.kernel.restore(&manifest.destination_path).unwrap();
    assert_eq!(
        rollback.validate(&proof, &budget),
        Err(ProofInvalidation::IncarnationChanged),
        "a restore invalidates every proof captured before it"
    );
}

#[test]
fn a_certificate_past_the_restored_kernel_tip_defeats_proof() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
    fixture.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let backup_dir = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(backup_dir.path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    let manifest = fixture
        .kernel
        .backup(BackupRequest {
            destination_directory: backup_dir.path().to_path_buf(),
            deadline: Instant::now() + Duration::from_secs(10),
            capture_pin_expires_at: None,
        })
        .unwrap();
    let restored_tip = fixture.tip();
    fixture.decide("later", &[ok("obj-2")]);
    fixture.project(&[], vec![]);
    let certificate = fixture.certificate();
    assert!(certificate.complete_through_commit_seq > restored_tip);
    fixture.kernel.restore(&manifest.destination_path).unwrap();
    assert_eq!(fixture.tip(), restored_tip);
    let budget = EvalBudget::unbounded();
    let resolution = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert_eq!(resolution.completion, Completion::Complete);
    assert_eq!(
        resolution.disqualified,
        Some(Disqualification::ProjectionLag {
            tip: restored_tip,
            complete_through: certificate.complete_through_commit_seq,
        }),
        "a projection past the restored tip may describe rolled-back history"
    );
    assert!(
        resolution.proof.is_none(),
        "the restored kernel cannot certify the projected horizon"
    );
    assert_eq!(
        resolution.retained.len(),
        1,
        "the eligible row still reaches hybrid"
    );
}

/// `validate_for_use` rejects a proof when the budget expires during its kernel batch.
#[test]
fn final_use_refuses_a_budget_that_expires_inside_the_kernel_batch() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
    fixture.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let certificate = fixture.certificate();
    let proof = fixture
        .resolve(
            object_query("obj-1"),
            true,
            &certificate,
            bounds(),
            &EvalBudget::unbounded(),
        )
        .unwrap()
        .proof
        .expect("a healthy singleton proves");
    let budget = EvalBudget::unbounded();
    let outcome = fixture
        .store
        .with_conn(|conn| {
            Ok(validate_for_use_with_hook_for_test(
                conn,
                &fixture.kernel,
                &proof,
                Authority {
                    project: &fixture.project,
                    destination: ArtifactDestination::Local,
                    inventory_epoch: EPOCH,
                },
                &budget,
                || budget.cancel(),
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(outcome, Err(ProofInvalidation::BudgetExhausted));
}

#[test]
fn a_budget_expiring_anywhere_inside_resolve_refuses_as_budget_exhaustion() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
    fixture.project(
        &[
            claim("obj-1", 1, "decision_summary"),
            claim("obj-1", 1, "rationale"),
        ],
        vec![],
    );
    let certificate = fixture.certificate();
    let one_row = ResolveBounds {
        page_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let unbounded = EvalBudget::unbounded();
    let started = Instant::now();
    fixture
        .resolve(
            object_query("obj-1"),
            true,
            &certificate,
            one_row,
            &unbounded,
        )
        .unwrap();
    let attempt = started.elapsed().max(Duration::from_micros(64));
    // Sweep deadlines from immediate to twice the observed resolve duration.
    for step in 0..512u32 {
        let deadline = Instant::now() + attempt.mul_f64(f64::from(step) / 256.0);
        let budget = EvalBudget::new(Some(deadline), Arc::new(AtomicBool::new(false)));
        match fixture.resolve(object_query("obj-1"), true, &certificate, one_row, &budget) {
            Ok(resolution) => assert!(
                matches!(
                    resolution.completion,
                    Completion::Complete
                        | Completion::Incomplete(IncompleteReason::BudgetExhausted)
                ),
                "a deadline may only stop an attempt as budget exhaustion: {:?}",
                resolution.completion
            ),
            Err(ResolveRefusal::BudgetExhausted) => {}
            Err(other) => panic!(
                "a deadline crossing mid-attempt must refuse as budget exhaustion: {other:?}"
            ),
        }
    }
}

#[test]
fn authority_churn_and_cancellation_during_enumeration_keep_progress_and_never_prove() {
    let fixture = Fixture::new();
    let objects: Vec<&'static str> = vec![
        "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9", "b0", "b1", "b2", "b3", "b4",
        "b5", "b6", "b7", "b8", "b9", "c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9",
        "d0", "d1",
    ];
    fixture
        .kernel
        .commit(intent("objects"), |envelope| {
            for (index, object) in objects.iter().enumerate() {
                let mut spec = decision(object, Some(SCOPE_A), Sensitivity::Normal);
                spec.source_revision = (index / 2) as i64 + 1;
                envelope.insert_decision(spec)?;
                envelope.record_admission(admission(object))?;
            }
            Ok(String::new())
        })
        .unwrap();
    let rows: Vec<Row> = objects
        .iter()
        .enumerate()
        .map(|(index, object)| Row::Claim {
            key: "churned",
            object,
            revision: 1,
            representation: ["decision_summary", "rationale"][index % 2],
            payload: object,
        })
        .collect();
    let rows: Vec<Row> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| match row {
            Row::Claim {
                key,
                object,
                representation,
                payload,
                ..
            } => Row::Claim {
                key,
                object,
                revision: (index / 2) as i64 + 1,
                representation,
                payload,
            },
            other => other,
        })
        .collect();
    fixture.project(&rows, vec![]);
    let one_row = ResolveBounds {
        page_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };

    let mut snapshot_changed = false;
    let mut cancelled_midway = false;
    let mut iteration = 0;
    while !(snapshot_changed && cancelled_midway) {
        assert!(
            iteration < 200,
            "authority churn never overlapped an attempt"
        );
        iteration += 1;
        fixture.project(&[], vec![]);
        let certificate = fixture.certificate();
        let interrupt = Arc::new(AtomicBool::new(false));
        let budget = EvalBudget::new(None, Arc::clone(&interrupt));
        let cancel = iteration % 3 == 2;
        let resolution = std::thread::scope(|scope| {
            let kernel = &fixture.kernel;
            let stop = Arc::new(AtomicBool::new(false));
            let writer_stop = Arc::clone(&stop);
            let writer_interrupt = Arc::clone(&interrupt);
            let writer = scope.spawn(move || {
                let mut commits = 0;
                while !writer_stop.load(Ordering::Relaxed) && commits < 64 {
                    kernel
                        .commit(
                            intent(&format!("churn-{iteration}-{commits}")),
                            |envelope| {
                                envelope.insert_decision(decision(
                                    &format!("churn-{iteration}-{commits}"),
                                    Some(SCOPE_A),
                                    Sensitivity::Normal,
                                ))?;
                                Ok(String::new())
                            },
                        )
                        .unwrap();
                    commits += 1;
                    if cancel && commits == 3 {
                        writer_interrupt.store(true, Ordering::Relaxed);
                    }
                }
            });
            let resolution = fixture.resolve(
                object_query("churned"),
                true,
                &certificate,
                one_row,
                &budget,
            );
            stop.store(true, Ordering::Relaxed);
            writer.join().unwrap();
            resolution
        });
        match resolution {
            Err(ResolveRefusal::BudgetExhausted) => {
                assert!(cancel, "only a cancelled budget refuses at entry");
            }
            Err(other) => panic!("{other:?}"),
            Ok(resolution) => {
                if let Some(proof) = &resolution.proof {
                    assert_eq!(
                        proof.snapshot().tip,
                        certificate.complete_through_commit_seq,
                        "a proof never covers a snapshot past the certified horizon"
                    );
                    assert_eq!(resolution.disqualified, None);
                }
                if resolution.disqualified == Some(Disqualification::SnapshotChanged)
                    || matches!(
                        resolution.disqualified,
                        Some(Disqualification::ProjectionLag { .. })
                    )
                {
                    snapshot_changed = true;
                    assert!(resolution.proof.is_none());
                }
                if resolution.completion
                    == Completion::Incomplete(IncompleteReason::BudgetExhausted)
                {
                    assert!(cancel);
                    cancelled_midway = true;
                    assert!(resolution.proof.is_none());
                }
                assert!(
                    resolution.retained.len() <= resolution.consumed.rows,
                    "retained progress never exceeds the rows read"
                );
                assert_eq!(resolution.retained.len(), resolution.observations.eligible);
                if resolution.completion == Completion::Complete {
                    assert_eq!(resolution.consumed.rows, rows.len());
                }
            }
        }
    }
    assert!(
        snapshot_changed,
        "the writer must have moved authority mid-attempt"
    );
    assert!(
        cancelled_midway,
        "cancellation must have landed mid-attempt"
    );
}
