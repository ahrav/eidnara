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
    KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope, ScopeSpec, ScopeTermSpec, Sensitivity,
    SourceClass, TaintClass,
};
use retrieval::batch::{
    BatchBounds, Invalidation, MutationIdentity, ProjectionBatch, apply_batch, read_checkpoint,
};
use retrieval::exact::{
    Authority, CertificateRefusal, CompletenessCertificate, Completion, Disqualification,
    ExactProof, ExactQuery, HexPrefix, IncompleteReason, LookupContext, LookupRefusal,
    ObjectFormat, ProofInvalidation, RequestIntent, Resolution, ResolveBounds, ResolveRefusal,
    ResolveRequest, ShaPrefixQuery, page, resolve, resolve_with_hook_for_test, validate_for_use,
    validate_for_use_with_hook_for_test,
};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity, Tombstone,
    TombstoneReason, install_identity,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
                        analysis_identity: retrieval::lexical::AnalysisIdentity::current()
                            .as_str()
                            .to_string(),
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
        self.project_citing(rows, invalidations, DIGEST);
    }

    /// Projects `rows` as derived from the artifact whose digest is `digest`.
    fn project_citing(&self, rows: &[Row], invalidations: Vec<Invalidation>, digest: &str) {
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
                source_artifact_digest: digest,
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
        let kernel_incarnation = self
            .kernel
            .capture_commit_read_target_within_budget(&EvalBudget::unbounded())
            .unwrap()
            .incarnation;
        CompletenessCertificate {
            canonical_incarnation_id: self.incarnation.clone(),
            kernel_incarnation,
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
/// association target, which is the key. A row retargeted by rewriting its
/// key, while its tuple still names the original object, must refuse the
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
                    "UPDATE exact_associations SET key=?2 WHERE key=?1",
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
        Err(ResolveRefusal::Lookup(LookupRefusal::Projection(
            ProjectionError::CorruptRow
        )))
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

/// A restore that completes after the final-use projection checks and before the final-use capture invalidates the proof.
#[test]
fn final_use_refuses_a_restore_that_lands_before_its_capture() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
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
    fixture.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();
    let proof = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap()
        .proof
        .expect("a healthy singleton proves");
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
                || {
                    fixture.kernel.restore(&manifest.destination_path).unwrap();
                },
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(outcome, Err(ProofInvalidation::IncarnationChanged));
}

#[test]
fn a_certificate_minted_past_the_restored_kernel_tip_is_refused() {
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
    assert_eq!(
        fixture
            .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
            .unwrap_err(),
        ResolveRefusal::Certificate(CertificateRefusal::IncarnationMismatch),
        "a certificate issued past the restored tip describes rolled-back history"
    );
}

/// An equal-tip restore changes the kernel incarnation, invalidating certificates issued against the displaced history.
#[test]
fn a_certificate_minted_before_an_equal_tip_restore_is_refused() {
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
    let certificate = fixture.certificate();
    let tip = fixture.tip();
    fixture.kernel.restore(&manifest.destination_path).unwrap();
    assert_eq!(fixture.tip(), tip, "the restore lands on the certified tip");
    let budget = EvalBudget::unbounded();
    assert_eq!(
        fixture
            .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
            .unwrap_err(),
        ResolveRefusal::Certificate(CertificateRefusal::IncarnationMismatch),
        "a certificate issued against the displaced history never reaches the rows"
    );
    let fresh = fixture.certificate();
    let resolution = fixture
        .resolve(object_query("obj-1"), true, &fresh, bounds(), &budget)
        .unwrap();
    assert!(
        resolution.proof.is_some(),
        "a certificate issued against the restored history proves again"
    );
}

/// `validate_for_use` rejects a proof when the budget expires before its final-use capture.
#[test]
fn final_use_refuses_a_budget_that_expires_before_its_capture() {
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
fn a_budget_expiring_inside_an_enumeration_kernel_batch_stops_with_progress_and_no_proof() {
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
    // The hook expires the budget after the page read passes its own budget check, so only the kernel batch can observe it.
    let run = |cancel_on_batch: Option<usize>| {
        let budget = EvalBudget::unbounded();
        let batches = std::cell::Cell::new(0);
        let hook = || {
            batches.set(batches.get() + 1);
            if Some(batches.get()) == cancel_on_batch {
                budget.cancel();
            }
        };
        let request = ResolveRequest {
            query: object_query("obj-1"),
            intent: RequestIntent::WholeRequest,
            certificate: &certificate,
            authority: Authority {
                project: &fixture.project,
                destination: ArtifactDestination::Local,
                inventory_epoch: EPOCH,
            },
            bounds: one_row,
        };
        fixture
            .store
            .with_conn(|conn| {
                Ok(resolve_with_hook_for_test(
                    conn,
                    &fixture.kernel,
                    &request,
                    &budget,
                    &hook,
                ))
            })
            .unwrap()
    };

    let healthy = run(None).unwrap();
    assert_eq!(healthy.completion, Completion::Complete);
    assert_eq!(healthy.retained.len(), 2);
    assert!(healthy.proof.is_some(), "the uncancelled control proves");

    let second = run(Some(2)).unwrap();
    assert_eq!(
        second.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert_eq!(second.retained.len(), 1, "the first page's verdict is kept");
    assert_eq!(second.consumed.validated, 1);
    assert_eq!(
        second.consumed.pages, 2,
        "the kernel batch stopped, not a later page read"
    );
    assert!(second.proof.is_none());

    let first = run(Some(1)).unwrap();
    assert_eq!(
        first.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted),
        "a read page whose batch never judged it is incomplete, never a match or absence"
    );
    assert!(first.retained.is_empty());
    assert_eq!(first.consumed.validated, 0);
    assert_eq!(first.consumed.pages, 1);
    assert!(first.proof.is_none());
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

/// What the association view runs inside the enumeration, at the row it is armed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interference {
    /// A kernel commit moves authority while the attempt is between its first and last page.
    Churn,
    /// The attempt's budget is cancelled.
    Cancel,
}

/// A request from the association view to the thread that owns the kernel handle.
enum Writer {
    Commit(String),
    Stop,
}

/// Stops the writer when the test body ends, including by a failed assertion, so the scope can join it.
struct StopWriter(std::sync::mpsc::SyncSender<Writer>);

impl Drop for StopWriter {
    fn drop(&mut self) {
        let _ = self.0.send(Writer::Stop);
    }
}

/// Bounds each wait between the association view and the writer, so a deadlock fails the test.
const WRITER_WAIT: Duration = Duration::from_secs(30);

#[test]
fn authority_churn_and_cancellation_during_enumeration_keep_progress_and_never_prove() {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

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
            revision: (index / 2) as i64 + 1,
            representation: ["decision_summary", "rationale"][index % 2],
            payload: object,
        })
        .collect();
    fixture.project(&rows, vec![]);
    let mut in_key_order: Vec<String> = rows.iter().map(Row::occurrence_id).collect();
    in_key_order.sort();
    let one_row = ResolveBounds {
        page_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };

    // Every association row a page statement steps passes through `interfere`;
    // each armed call runs its interference before the statement yields the row.
    let stepped = Arc::new(AtomicUsize::new(0));
    let armed: Arc<Mutex<Vec<(usize, Interference)>>> = Arc::new(Mutex::new(Vec::new()));
    let budget_slot: Arc<Mutex<Option<EvalBudget>>> = Arc::new(Mutex::new(None));
    let (requests, writer_requests) = mpsc::sync_channel::<Writer>(0);
    let (writer_done, done) = mpsc::sync_channel::<()>(0);
    let stop = requests.clone();
    {
        let stepped = Arc::clone(&stepped);
        let armed = Arc::clone(&armed);
        let budget_slot = Arc::clone(&budget_slot);
        let done = Mutex::new(done);
        // A repeated intent replays without committing, so every churn names a new one.
        let churns = AtomicUsize::new(0);
        fixture
            .store
            .with_conn_unfenced(|conn| {
                conn.create_scalar_function(
                    "interfere",
                    1,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8
                        | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
                    move |ctx| {
                        let call = stepped.fetch_add(1, Ordering::Relaxed) + 1;
                        let fired: Vec<Interference> = {
                            let mut armed = armed.lock().unwrap();
                            let fired = armed
                                .iter()
                                .filter(|(at, _)| *at == call)
                                .map(|(_, interference)| *interference)
                                .collect();
                            armed.retain(|(at, _)| *at != call);
                            fired
                        };
                        for interference in fired {
                            match interference {
                                Interference::Churn => {
                                    let churn = churns.fetch_add(1, Ordering::Relaxed);
                                    requests
                                        .send(Writer::Commit(format!("churn-{churn}")))
                                        .unwrap();
                                    done.lock()
                                        .unwrap()
                                        .recv_timeout(WRITER_WAIT)
                                        .expect("the writer commits within the bound");
                                }
                                Interference::Cancel => {
                                    budget_slot.lock().unwrap().as_ref().unwrap().cancel();
                                }
                            }
                        }
                        ctx.get::<i64>(0)
                    },
                )?;
                conn.execute_batch(
                    "ALTER TABLE exact_associations RENAME TO original_associations;
                     CREATE VIEW exact_associations AS SELECT family,namespace,key,occurrence_id,
                         extraction_version,interfere(created_commit_seq) AS created_commit_seq
                     FROM original_associations;",
                )
            })
            .unwrap();
    }

    std::thread::scope(|scope| {
        let _stop = StopWriter(stop);
        let kernel = &fixture.kernel;
        scope.spawn(move || {
            while let Ok(Writer::Commit(key)) = writer_requests.recv() {
                kernel
                    .commit(intent(&key), |envelope| {
                        envelope.insert_decision(decision(
                            &key,
                            Some(SCOPE_A),
                            Sensitivity::Normal,
                        ))?;
                        Ok(String::new())
                    })
                    .unwrap();
                writer_done.send(()).unwrap();
            }
        });
        // A page of one row steps that row and one more to learn whether a next page exists,
        // so page k steps rows 2k-1 and 2k: call 1 lands in the first page, and calls 4 and 21
        // land after one and ten pages were judged.
        let cases: [&[(usize, Interference)]; 8] = [
            &[],
            &[(1, Interference::Churn)],
            &[(4, Interference::Churn)],
            &[(21, Interference::Churn)],
            &[(1, Interference::Cancel)],
            &[(4, Interference::Cancel)],
            &[(21, Interference::Cancel)],
            &[(4, Interference::Churn), (21, Interference::Cancel)],
        ];
        for interferences in cases {
            fixture.project(&[], vec![]);
            let certificate = fixture.certificate();
            let budget = EvalBudget::new(None, Arc::new(AtomicBool::new(false)));
            stepped.store(0, Ordering::Relaxed);
            *budget_slot.lock().unwrap() = Some(budget.clone());
            *armed.lock().unwrap() = interferences.to_vec();
            let resolution = fixture.resolve(
                object_query("churned"),
                true,
                &certificate,
                one_row,
                &budget,
            );
            let case = format!("{interferences:?}");
            assert!(
                armed.lock().unwrap().is_empty(),
                "{case}: every interference must fire inside the attempt"
            );
            let churned = interferences
                .iter()
                .any(|(_, interference)| *interference == Interference::Churn);
            let cancelled_at = interferences
                .iter()
                .find(|(_, interference)| *interference == Interference::Cancel)
                .map(|(at, _)| *at);
            if cancelled_at == Some(1) {
                assert_eq!(
                    resolution.unwrap_err(),
                    ResolveRefusal::BudgetExhausted,
                    "{case}: a cancellation before any page completes refuses the attempt"
                );
                continue;
            }
            let resolution = resolution.unwrap_or_else(|refusal| panic!("{case}: {refusal:?}"));
            assert_eq!(resolution.retained.len(), resolution.observations.eligible);
            let retained: Vec<&str> = resolution
                .retained
                .iter()
                .map(|row| row.occurrence_id.as_str())
                .collect();
            let read = match cancelled_at {
                Some(at) => {
                    assert_eq!(
                        resolution.completion,
                        Completion::Incomplete(IncompleteReason::BudgetExhausted),
                        "{case}: cancellation landed mid-attempt"
                    );
                    (at - 1) / 2
                }
                None => {
                    assert_eq!(resolution.completion, Completion::Complete, "{case}");
                    rows.len()
                }
            };
            assert_eq!(resolution.consumed.pages, read, "{case}");
            assert_eq!(resolution.consumed.rows, read, "{case}");
            assert_eq!(
                retained,
                in_key_order[..read],
                "{case}: every row read before the interference is kept, in key order"
            );
            if churned {
                assert_eq!(
                    resolution.disqualified,
                    Some(Disqualification::SnapshotChanged),
                    "{case}: the writer moved authority mid-attempt"
                );
            }
            if churned || cancelled_at.is_some() {
                assert!(resolution.proof.is_none(), "{case}");
                continue;
            }
            assert_eq!(resolution.disqualified, None, "{case}");
            let proof = resolution
                .proof
                .expect("an undisturbed attempt over one target proves");
            assert_eq!(
                proof.snapshot().tip,
                certificate.complete_through_commit_seq,
                "a proof never covers a snapshot past the certified horizon"
            );
        }
    });
}

/// Projects `count` claim rows under the one exact key `bulk`, each naming an object the kernel never decided, so every row is judged and none proves.
fn project_bulk(fixture: &Fixture, count: usize) {
    // Each row carries its own revision, so the shared key yields distinct occurrences.
    let objects: Vec<&'static str> = (0..count)
        .map(|n| &*Box::leak(format!("bulk-{n:05}").into_boxed_str()))
        .collect();
    let rows: Vec<Row> = objects
        .iter()
        .enumerate()
        .map(|(index, object)| Row::Claim {
            key: "bulk",
            object,
            revision: index as i64 + 1,
            representation: "decision_summary",
            payload: object,
        })
        .collect();
    for chunk in rows.chunks(200) {
        fixture.project(chunk, vec![]);
    }
}

const BULK_ROWS: usize = 3000;

fn bulk_bounds() -> ResolveBounds {
    ResolveBounds {
        page_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        max_rows: NonZeroUsize::new(4 * MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        max_pages: NonZeroUsize::new(8).unwrap(),
        ..bounds()
    }
}

impl Fixture {
    /// Resolves `bulk` under `budget` on a read whose progress handler polls `stop`.
    fn resolve_interruptible(
        &self,
        certificate: &CompletenessCertificate,
        budget: &EvalBudget,
        stop: impl FnMut() -> bool + Send + 'static,
    ) -> Result<Resolution, ResolveRefusal> {
        let request = ResolveRequest {
            query: object_query("bulk"),
            intent: RequestIntent::WholeRequest,
            certificate,
            authority: Authority {
                project: &self.project,
                destination: ArtifactDestination::Local,
                inventory_epoch: EPOCH,
            },
            bounds: bulk_bounds(),
        };
        self.store
            .with_conn_interruptible(Instant::now() + Duration::from_secs(60), stop, |conn| {
                Ok(resolve(conn, &self.kernel, &request, budget))
            })
            .unwrap()
    }
}

/// A progress-handler stop that counts its polls and, from poll `fire_at`, cancels `budget` and interrupts the statement it is polled from.
fn cancelling_stop(
    budget: &EvalBudget,
    fire_at: usize,
) -> (Arc<AtomicUsize>, impl FnMut() -> bool + Send + 'static) {
    let polls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&polls);
    let budget = budget.clone();
    let stop = move || {
        if counted.fetch_add(1, Ordering::Relaxed) + 1 >= fire_at {
            budget.cancel();
        }
        budget.is_exhausted()
    };
    (polls, stop)
}

#[test]
fn an_interrupt_inside_an_exact_page_statement_stops_resolution_as_budget_exhaustion() {
    let fixture = Fixture::new();
    project_bulk(&fixture, BULK_ROWS);
    let certificate = fixture.certificate();

    // Healthy control: the same interruptible path completes and the handler polls inside each page statement.
    let budget = EvalBudget::unbounded();
    let polls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&polls);
    let control = fixture
        .resolve_interruptible(&certificate, &budget, move || {
            counted.fetch_add(1, Ordering::Relaxed);
            false
        })
        .unwrap();
    assert_eq!(control.completion, Completion::Complete);
    assert_eq!(control.consumed.rows, BULK_ROWS);
    assert_eq!(control.consumed.pages, 3);
    assert_eq!(control.consumed.validated, BULK_ROWS);
    assert!(control.proof.is_none(), "undecided objects never prove");
    let total = polls.load(Ordering::Relaxed);
    assert!(
        total >= 6,
        "the handler polls inside the page statements: {total}"
    );

    // A page read alone surfaces the interrupt as an engine interruption, not as a budget check.
    let budget = EvalBudget::unbounded();
    let (fired, stop) = cancelling_stop(&budget, 2);
    let refused = fixture
        .store
        .with_conn_interruptible(Instant::now() + Duration::from_secs(60), stop, |conn| {
            Ok(page(
                conn,
                &LookupContext {
                    kernel_incarnation_id: &fixture.incarnation,
                    page_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
                    budget: &budget,
                },
                &object_query("bulk"),
                None,
            ))
        })
        .unwrap();
    assert!(fired.load(Ordering::Relaxed) >= 2, "the stop fired");
    assert_eq!(
        refused.unwrap_err(),
        LookupRefusal::Projection(ProjectionError::Interrupted)
    );

    // The first poll is the connection acquisition, which refuses before any SQL; interrupting at every later poll position stops the attempt as a global or local budget stop, keeping completed pages only.
    let mut global = false;
    let mut local = false;
    for fire_at in 2..=total {
        let budget = EvalBudget::unbounded();
        let (polled, stop) = cancelling_stop(&budget, fire_at);
        let outcome = fixture.resolve_interruptible(&certificate, &budget, stop);
        assert!(
            polled.load(Ordering::Relaxed) >= fire_at.min(total),
            "{fire_at}: the stop was polled up to its firing point"
        );
        match outcome {
            Err(ResolveRefusal::BudgetExhausted) => global = true,
            Ok(resolution) => {
                assert_eq!(
                    resolution.completion,
                    Completion::Incomplete(IncompleteReason::BudgetExhausted),
                    "{fire_at}"
                );
                assert!(resolution.proof.is_none(), "{fire_at}");
                assert!(resolution.consumed.pages >= 1, "{fire_at}");
                assert_eq!(
                    resolution.consumed.validated, resolution.consumed.rows,
                    "{fire_at}: accounting covers judged pages only"
                );
                assert_eq!(
                    resolution.consumed.rows,
                    resolution.consumed.pages * MAX_ELIGIBILITY_CANDIDATES,
                    "{fire_at}: an interrupted page is dropped unjudged"
                );
                local = true;
            }
            Err(other) => {
                panic!("{fire_at}: an interrupt must stop as budget exhaustion: {other:?}")
            }
        }
    }
    assert!(
        global && local,
        "the sweep reaches both the global and the local stop"
    );

    // The connection is reusable: the interrupted requests leave no handler behind, and a late cancellation of an earlier request's budget cannot stop a later one.
    let earlier = EvalBudget::unbounded();
    let (_, stop) = cancelling_stop(&earlier, 2);
    fixture
        .resolve_interruptible(&certificate, &earlier, stop)
        .unwrap_err();
    earlier.cancel();
    let later = EvalBudget::unbounded();
    let plain = fixture
        .resolve_with(
            ResolveRequest {
                query: object_query("bulk"),
                intent: RequestIntent::WholeRequest,
                certificate: &certificate,
                authority: Authority {
                    project: &fixture.project,
                    destination: ArtifactDestination::Local,
                    inventory_epoch: EPOCH,
                },
                bounds: bulk_bounds(),
            },
            &later,
        )
        .unwrap();
    assert_eq!(plain.completion, Completion::Complete);
    let after = fixture
        .resolve_interruptible(&certificate, &later, || false)
        .unwrap();
    assert_eq!(after.completion, Completion::Complete);
}

#[test]
fn a_deadline_reached_inside_an_exact_page_statement_interrupts_it() {
    let fixture = Fixture::new();
    project_bulk(&fixture, BULK_ROWS);
    let certificate = fixture.certificate();
    // The handler waits out the deadline at its second poll, the first one inside the page statement, so the budget ends while that statement runs.
    let deadline_stop = || {
        let deadline = Instant::now() + Duration::from_millis(300);
        let budget = EvalBudget::new(Some(deadline), Arc::new(AtomicBool::new(false)));
        let polls = Arc::new(AtomicUsize::new(0));
        let stop = {
            let budget = budget.clone();
            let polls = Arc::clone(&polls);
            move || {
                if polls.fetch_add(1, Ordering::Relaxed) + 1 == 2 {
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                }
                budget.is_exhausted()
            }
        };
        (budget, polls, stop)
    };

    let (budget, polls, stop) = deadline_stop();
    let refused = fixture
        .store
        .with_conn_interruptible(Instant::now() + Duration::from_secs(60), stop, |conn| {
            Ok(page(
                conn,
                &LookupContext {
                    kernel_incarnation_id: &fixture.incarnation,
                    page_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
                    budget: &budget,
                },
                &object_query("bulk"),
                None,
            ))
        })
        .unwrap();
    assert!(polls.load(Ordering::Relaxed) >= 2);
    assert!(budget.is_exhausted());
    assert_eq!(
        refused.unwrap_err(),
        LookupRefusal::Projection(ProjectionError::Interrupted),
        "the statement was interrupted, not refused by a budget check"
    );

    let (budget, polls, stop) = deadline_stop();
    let outcome = fixture.resolve_interruptible(&certificate, &budget, stop);
    assert!(polls.load(Ordering::Relaxed) >= 2);
    assert_eq!(
        outcome.unwrap_err(),
        ResolveRefusal::BudgetExhausted,
        "the first page never completed, so the stop is global"
    );
}

#[test]
fn an_engine_failure_while_the_budget_ends_stays_an_engine_failure() {
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
    let ended = Arc::new(AtomicBool::new(false));
    let end = Arc::clone(&ended);
    // The view's function ends the budget and fails the same SQLite step with an ordinary engine error.
    fixture
        .store
        .with_conn_unfenced(|conn| {
            conn.create_scalar_function(
                "fail_lookup",
                1,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8
                    | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
                move |_| -> rusqlite::Result<i64> {
                    end.store(true, Ordering::Relaxed);
                    Err(rusqlite::Error::UserFunctionError(
                        "injected engine failure".into(),
                    ))
                },
            )?;
            conn.execute_batch(
                "ALTER TABLE exact_associations RENAME TO original_associations;
                 CREATE VIEW exact_associations AS SELECT family,namespace,key,occurrence_id,
                     extraction_version,fail_lookup(created_commit_seq) AS created_commit_seq
                 FROM original_associations;",
            )
        })
        .unwrap();
    for interruptible in [false, true] {
        ended.store(false, Ordering::Relaxed);
        let budget = EvalBudget::new(None, Arc::clone(&ended));
        let request = ResolveRequest {
            query: object_query("obj-1"),
            intent: RequestIntent::WholeRequest,
            certificate: &certificate,
            authority: Authority {
                project: &fixture.project,
                destination: ArtifactDestination::Local,
                inventory_epoch: EPOCH,
            },
            bounds: bounds(),
        };
        let outcome = if interruptible {
            let stopping = budget.clone();
            fixture
                .store
                .with_conn_interruptible(
                    Instant::now() + Duration::from_secs(60),
                    move || stopping.is_exhausted(),
                    |conn| Ok(resolve(conn, &fixture.kernel, &request, &budget)),
                )
                .unwrap()
        } else {
            fixture.resolve_with(request, &budget)
        };
        assert!(budget.is_exhausted(), "the failing step ended the budget");
        assert!(
            matches!(
                outcome,
                Err(ResolveRefusal::Lookup(LookupRefusal::Projection(
                    ProjectionError::Sqlite(_)
                )))
            ),
            "interruptible={interruptible}: {outcome:?}"
        );
    }
}

/// An artifact ingest that a replay with a stricter `sensitivity` reclassifies in place.
fn classified_artifact(sensitivity: Sensitivity) -> kernel::ArtifactIngestRequest {
    kernel::ArtifactIngestRequest {
        intent: intent("artifact"),
        payload: b"observed bytes".to_vec(),
        evidence_id: "evidence-artifact".to_string(),
        object_id: "evidence-object-artifact".to_string(),
        object_kind: "evidence".to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "repository".to_string(),
        source_id: "src/artifact".to_string(),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: "canonical".to_string(),
        retain_until: None,
        asserted_sensitivity: sensitivity,
        provider_egress: kernel::ProviderEgress::RemoteAllowed,
        provenance: Some(kernel::RepositoryProvenance {
            repository_id: "repo".to_string(),
            revision: "abc123".to_string(),
        }),
    }
}

#[test]
fn a_classification_change_defeats_the_attempt_and_the_proof_whether_or_not_the_tip_moves() {
    let classified = || {
        let fixture = Fixture::new();
        fixture.decide("objects", &[ok("obj-1")]);
        let digest = fixture
            .kernel
            .ingest_artifact(classified_artifact(Sensitivity::Normal))
            .unwrap()
            .digest;
        fixture.project_citing(
            &[
                claim("obj-1", 1, "decision_summary"),
                claim("obj-1", 1, "rationale"),
            ],
            vec![],
            &digest,
        );
        fixture
    };
    let tighten = |fixture: &Fixture| {
        let tip = fixture.tip();
        fixture
            .kernel
            .ingest_artifact(classified_artifact(Sensitivity::Sensitive))
            .unwrap();
        // The replay commits, so a reclassification never leaves the tip unchanged.
        assert!(fixture.tip() > tip, "the reclassification moves the tip");
    };

    let fixture = classified();
    let certificate = fixture.certificate();
    let request = ResolveRequest {
        query: object_query("obj-1"),
        intent: RequestIntent::WholeRequest,
        certificate: &certificate,
        authority: Authority {
            project: &fixture.project,
            destination: ArtifactDestination::Local,
            inventory_epoch: EPOCH,
        },
        bounds: ResolveBounds {
            page_rows: NonZeroUsize::new(1).unwrap(),
            ..bounds()
        },
    };
    let run = |hook: &dyn Fn()| {
        fixture
            .store
            .with_conn(|conn| {
                Ok(resolve_with_hook_for_test(
                    conn,
                    &fixture.kernel,
                    &request,
                    &EvalBudget::unbounded(),
                    hook,
                ))
            })
            .unwrap()
            .unwrap()
    };
    assert!(run(&|| ()).proof.is_some(), "the unchanged control proves");

    // A classification window open across the second batch leaves that snapshot without a reusable generation.
    let window = std::cell::RefCell::new(None);
    let batches = std::cell::Cell::new(0);
    let hold = || {
        batches.set(batches.get() + 1);
        if batches.get() == 2 {
            *window.borrow_mut() = Some(fixture.kernel.hold_classification_change_for_test());
        }
    };
    let unknown = run(&hold);
    assert!(window.borrow().is_some());
    assert_eq!(
        unknown.disqualified,
        Some(Disqualification::ClassificationUnknown)
    );
    assert!(unknown.proof.is_none());
    drop(window.into_inner());
    assert!(
        run(&|| ()).proof.is_some(),
        "closing the window restores the proof"
    );

    // Between enumeration batches, the moved tip disqualifies the attempt.
    let batches = std::cell::Cell::new(0);
    let hook = || {
        batches.set(batches.get() + 1);
        if batches.get() == 2 {
            tighten(&fixture);
        }
    };
    let resolution = run(&hook);
    assert_eq!(batches.get(), 2);
    assert_eq!(resolution.completion, Completion::Complete);
    assert_eq!(
        resolution.disqualified,
        Some(Disqualification::SnapshotChanged)
    );
    assert!(resolution.proof.is_none());

    // After a proof is minted, final use sees the moved tip.
    let fixture = classified();
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
    assert_eq!(fixture.validate(&proof, &EvalBudget::unbounded()), Ok(()));
    tighten(&fixture);
    assert!(
        matches!(
            fixture.validate(&proof, &EvalBudget::unbounded()),
            Err(ProofInvalidation::CanonicalChanged { .. })
        ),
        "the proof names the tip before the reclassification"
    );

    // A proof judged while a classification window is open fails final use at the same tip.
    let fixture = classified();
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
    let tip = fixture.tip();
    let window = fixture.kernel.hold_classification_change_for_test();
    assert_eq!(
        fixture.validate(&proof, &EvalBudget::unbounded()),
        Err(ProofInvalidation::SnapshotChanged)
    );
    assert_eq!(fixture.tip(), tip, "the window moves no tip");
    drop(window);
    assert_eq!(
        fixture.validate(&proof, &EvalBudget::unbounded()),
        Err(ProofInvalidation::SnapshotChanged),
        "the closed window leaves a new generation the proof never judged"
    );
}

#[test]
fn an_interrupted_admission_or_final_use_read_is_budget_exhaustion() {
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
    // The view returns the stored checkpoint after enough work that the progress handler polls inside the checkpoint read.
    fixture
        .store
        .with_conn_unfenced(|conn| {
            conn.execute_batch(
                "ALTER TABLE projection_checkpoint RENAME TO original_checkpoint;
                 CREATE VIEW projection_checkpoint AS SELECT singleton,snapshot_commit_seq,
                     checkpoint_commit_seq + (WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL
                         SELECT i+1 FROM n WHERE i<20000) SELECT count(*)-20000 FROM n)
                         AS checkpoint_commit_seq,hold_id,updated_at
                 FROM original_checkpoint;",
            )
        })
        .unwrap();
    let request = ResolveRequest {
        query: object_query("obj-1"),
        intent: RequestIntent::WholeRequest,
        certificate: &certificate,
        authority: Authority {
            project: &fixture.project,
            destination: ArtifactDestination::Local,
            inventory_epoch: EPOCH,
        },
        bounds: bounds(),
    };
    let interruptible = |budget: &EvalBudget, fire_at: Option<usize>| {
        let (polls, stop) = cancelling_stop(budget, fire_at.unwrap_or(usize::MAX));
        let resolved = fixture
            .store
            .with_conn_interruptible(Instant::now() + Duration::from_secs(60), stop, |conn| {
                Ok(resolve(conn, &fixture.kernel, &request, budget))
            })
            .unwrap();
        (polls.load(Ordering::Relaxed), resolved)
    };
    let validated = |budget: &EvalBudget, fire_at: Option<usize>| {
        let (polls, stop) = cancelling_stop(budget, fire_at.unwrap_or(usize::MAX));
        let outcome = fixture
            .store
            .with_conn_interruptible(Instant::now() + Duration::from_secs(60), stop, |conn| {
                Ok(validate_for_use(
                    conn,
                    &fixture.kernel,
                    &proof,
                    Authority {
                        project: &fixture.project,
                        destination: ArtifactDestination::Local,
                        inventory_epoch: EPOCH,
                    },
                    budget,
                ))
            })
            .unwrap();
        (polls.load(Ordering::Relaxed), outcome)
    };

    let (polls, healthy) = interruptible(&EvalBudget::unbounded(), None);
    assert!(polls >= 3, "the checkpoint read polls: {polls}");
    assert!(
        healthy.unwrap().proof.is_some(),
        "the uninterrupted control proves"
    );
    let (_, interrupted) = interruptible(&EvalBudget::unbounded(), Some(2));
    assert_eq!(
        interrupted.unwrap_err(),
        ResolveRefusal::BudgetExhausted,
        "an interrupt inside certificate admission is the global stop"
    );

    let (polls, healthy) = validated(&EvalBudget::unbounded(), None);
    assert!(polls >= 2, "the final-use checkpoint read polls: {polls}");
    assert_eq!(healthy.unwrap(), Ok(()));
    let (_, interrupted) = validated(&EvalBudget::unbounded(), Some(2));
    assert_eq!(
        interrupted.unwrap(),
        Err(ProofInvalidation::BudgetExhausted)
    );
}

/// Judges `candidates` one kernel batch each, independently of the adapter's batching.
fn judged_alone(
    fixture: &Fixture,
    candidates: &[retrieval::eligibility::OccurrenceCandidate],
) -> Vec<EligibilityVerdict> {
    candidates
        .iter()
        .map(|candidate| {
            let batch = fixture
                .kernel
                .judge_eligibility(
                    &fixture.project,
                    ArtifactDestination::Local,
                    std::slice::from_ref(&candidate.candidate),
                )
                .unwrap();
            assert_eq!(batch.verdicts.len(), 1);
            batch.verdicts[0]
        })
        .collect()
}

#[test]
fn equal_kernel_terms_share_one_verdict_and_every_differing_term_is_judged_on_its_own() {
    use kernel::KernelError;
    use kernel::source_identity::OccurrenceClass;
    use retrieval::eligibility::{
        Disposition, OccurrenceCandidate, judge_dispositions_within_budget,
        judge_occurrences_within_budget,
    };

    let fixture = Fixture::new();
    fixture.decide(
        "objects",
        &[
            ok("obj-1"),
            ok("obj-2"),
            ("elsewhere", Some(SCOPE_B), Sensitivity::Normal, true),
        ],
    );
    let other_digest = "1".repeat(64);
    let terms: [(&str, &str, i64, &str); 8] = [
        ("occ-a", "obj-1", 1, DIGEST),
        ("occ-b", "obj-1", 1, DIGEST),
        ("occ-c", "obj-1", 2, DIGEST),
        ("occ-d", "obj-1", 1, &other_digest),
        ("occ-e", "elsewhere", 1, DIGEST),
        ("occ-f", "obj-1", 1, DIGEST),
        ("occ-g", "obj-2", 1, DIGEST),
        ("occ-b", "obj-1", 1, DIGEST),
    ];
    let candidates: Vec<OccurrenceCandidate> = terms
        .iter()
        .map(|(occurrence, object, revision, digest)| {
            OccurrenceCandidate::new(
                occurrence.to_string(),
                OccurrenceClass::CanonicalClaims,
                object.to_string(),
                *revision,
                digest.to_string(),
            )
        })
        .collect();
    let oracle = judged_alone(&fixture, &candidates);
    assert_eq!(oracle[0], EligibilityVerdict::Ok);
    assert_eq!(
        oracle[2],
        EligibilityVerdict::Stale,
        "another revision of an eligible object borrows no authorization"
    );
    assert_eq!(oracle[4], EligibilityVerdict::WrongScope);

    let budget = EvalBudget::unbounded();
    let report = judge_occurrences_within_budget(
        &fixture.kernel,
        &fixture.project,
        ArtifactDestination::Local,
        &candidates,
        &budget,
    )
    .unwrap();
    assert_eq!(report.occurrences.len(), candidates.len());
    for ((judged, candidate), verdict) in report.occurrences.iter().zip(&candidates).zip(&oracle) {
        assert_eq!(judged.occurrence_id, candidate.occurrence_id);
        assert_eq!(judged.class, candidate.class);
        assert_eq!(judged.disposition, Disposition::from(*verdict));
    }
    let dispositions = judge_dispositions_within_budget(
        &fixture.kernel,
        &fixture.project,
        ArtifactDestination::Local,
        &candidates,
        &budget,
    )
    .unwrap();
    assert_eq!(dispositions.snapshot, report.snapshot);
    assert_eq!(dispositions.incarnation, report.incarnation);
    assert_eq!(
        dispositions.dispositions,
        oracle
            .iter()
            .copied()
            .map(Disposition::from)
            .collect::<Vec<_>>()
    );

    let at_bound = vec![candidates[0].clone(); MAX_ELIGIBILITY_CANDIDATES];
    let judged = judge_dispositions_within_budget(
        &fixture.kernel,
        &fixture.project,
        ArtifactDestination::Local,
        &at_bound,
        &budget,
    )
    .unwrap();
    assert_eq!(judged.dispositions.len(), MAX_ELIGIBILITY_CANDIDATES);
    assert!(
        judged
            .dispositions
            .iter()
            .all(|disposition| *disposition == Disposition::Eligible)
    );
    let over = vec![candidates[0].clone(); MAX_ELIGIBILITY_CANDIDATES + 1];
    assert_eq!(
        judge_occurrences_within_budget(
            &fixture.kernel,
            &fixture.project,
            ArtifactDestination::Local,
            &over,
            &budget,
        )
        .unwrap_err(),
        KernelError::InvalidInput,
        "duplicates count against the kernel's batch bound"
    );
    assert_eq!(
        judge_dispositions_within_budget(
            &fixture.kernel,
            &fixture.project,
            ArtifactDestination::Local,
            &over,
            &budget,
        )
        .unwrap_err(),
        KernelError::InvalidInput
    );
}

#[test]
fn a_projection_under_another_analysis_identity_refuses_at_its_first_page() {
    let fixture = Fixture::new();
    fixture.decide("objects", &[ok("obj-1")]);
    fixture.project(&[claim("obj-1", 1, "decision_summary")], vec![]);
    let certificate = fixture.certificate();
    let budget = EvalBudget::unbounded();
    let healthy = fixture
        .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
        .unwrap();
    assert!(healthy.proof.is_some(), "the unaltered projection proves");

    fixture
        .store
        .with_conn_unfenced(|conn| {
            conn.execute(
                "UPDATE projection_identity SET analysis_identity='another-analysis'",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    assert_eq!(
        fixture
            .resolve(object_query("obj-1"), true, &certificate, bounds(), &budget)
            .unwrap_err(),
        ResolveRefusal::Lookup(LookupRefusal::SchemaMismatch),
        "the admitted snapshot's analysis identity refuses the first page"
    );
    let context = LookupContext {
        kernel_incarnation_id: &fixture.incarnation,
        page_rows: NonZeroUsize::new(2).unwrap(),
        budget: &budget,
    };
    assert_eq!(
        fixture
            .store
            .with_conn(|conn| Ok(page(conn, &context, &object_query("obj-1"), None)))
            .unwrap()
            .unwrap_err(),
        LookupRefusal::SchemaMismatch,
        "a direct page read refuses the same snapshot alike"
    );
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
}
