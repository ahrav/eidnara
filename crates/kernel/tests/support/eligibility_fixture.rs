//! The eligibility store fixture: one object per verdict class and
//! precedence pair, shared by the kernel eligibility table and the
//! evaluator's spec differential.
//!
//! `automatic` is admitted through a triggered `CodeObserved` from trusted
//! local code, which the admission policy serves on every surface; every
//! other admitted object is served labeled on explicit search only.

#![allow(dead_code)]

use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactIngestRequest, CommitIntent, DecisionPayload,
    DecisionSpec, Dimension, DomainSpec, EligibilityCandidate, EventKind, KernelStore,
    ProviderEgress, RepositoryProvenance, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass,
    TaintClass,
};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
pub const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const PROJECT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const SCOPE_A: &str = "project:a";
pub const SCOPE_B: &str = "project:b";
const SCOPE_NO_PROJECT: &str = "scope:branch-only";

pub fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-eligibility-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

pub fn project_term(digest: &str) -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: Dimension::Project.as_str().to_string(),
        operator: "exact".to_string(),
        exact_value: Some(digest.to_string()),
        ..ScopeTermSpec::default()
    }
}

pub fn branch_term() -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: Dimension::Branch.as_str().to_string(),
        operator: "exact".to_string(),
        exact_value: Some("main".to_string()),
        ..ScopeTermSpec::default()
    }
}

fn scope(scope_id: &str, terms: Vec<ScopeTermSpec>) -> ScopeSpec {
    ScopeSpec {
        scope_id: scope_id.to_string(),
        object_id: scope_id.to_string(),
        source_id: scope_id.to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "kernel_route".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms,
    }
}

pub fn decision(object: &str, scope_id: Option<&str>, sensitivity: Sensitivity) -> DecisionSpec {
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

pub fn admission(object: &str, kind: EventKind) -> AdmissionRequest {
    admission_from(
        object,
        kind,
        SourceClass::ExplicitUser,
        TaintClass::UserExplicit,
    )
}

fn admission_from(
    object: &str,
    kind: EventKind,
    source_class: SourceClass,
    taint_class: TaintClass,
) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object.to_string()),
        source_class: Some(source_class),
        taint_class: Some(taint_class),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "test".to_string(),
        },
    }
}

pub fn artifact(key: &str, payload: &[u8], sensitivity: Sensitivity) -> ArtifactIngestRequest {
    ArtifactIngestRequest {
        intent: intent(key),
        payload: payload.to_vec(),
        evidence_id: format!("evidence-{key}"),
        object_id: format!("evidence-object-{key}"),
        object_kind: "evidence".to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "repository".to_string(),
        source_id: format!("src/{key}"),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: "canonical".to_string(),
        retain_until: None,
        asserted_sensitivity: sensitivity,
        provider_egress: ProviderEgress::RemoteAllowed,
        // A `Normal` assertion without provenance is stored as `Sensitive`.
        provenance: Some(RepositoryProvenance {
            repository_id: "repo".to_string(),
            revision: "abc123".to_string(),
        }),
    }
}

pub fn candidate(object: &str, revision: i64) -> EligibilityCandidate {
    EligibilityCandidate {
        object_id: object.to_string(),
        source_revision: revision,
        artifact_digest: None,
    }
}

pub fn with_artifact(mut candidate: EligibilityCandidate, digest: &str) -> EligibilityCandidate {
    candidate.artifact_digest = Some(digest.to_string());
    candidate
}

pub struct Fixture {
    pub _root: tempfile::TempDir,
    pub store: KernelStore,
    pub sensitive_artifact: String,
    /// Denied for every destination.
    pub secret_artifact: String,
}

pub fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    store
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
            envelope.insert_scope(scope(SCOPE_A, vec![project_term(PROJECT_A)]))?;
            envelope.insert_scope(scope(SCOPE_B, vec![project_term(PROJECT_B)]))?;
            envelope.insert_scope(scope(SCOPE_NO_PROJECT, vec![branch_term()]))?;
            for (object, scope_id, sensitivity) in [
                ("ok", Some(SCOPE_A), Sensitivity::Normal),
                ("retired", Some(SCOPE_A), Sensitivity::Normal),
                ("replaced", Some(SCOPE_A), Sensitivity::Normal),
                ("stale", Some(SCOPE_A), Sensitivity::Normal),
                ("other-project", Some(SCOPE_B), Sensitivity::Normal),
                (
                    "branch-only-scope",
                    Some(SCOPE_NO_PROJECT),
                    Sensitivity::Normal,
                ),
                ("unscoped", None, Sensitivity::Normal),
                ("secret", Some(SCOPE_A), Sensitivity::Secret),
                ("sensitive", Some(SCOPE_A), Sensitivity::Sensitive),
                ("unadmitted", Some(SCOPE_A), Sensitivity::Normal),
                ("contradicted", Some(SCOPE_A), Sensitivity::Normal),
                ("with-artifact", Some(SCOPE_A), Sensitivity::Normal),
                ("retired-other-project", Some(SCOPE_B), Sensitivity::Normal),
                ("stale-other-project", Some(SCOPE_B), Sensitivity::Normal),
                ("secret-other-project", Some(SCOPE_B), Sensitivity::Secret),
                ("secret-unadmitted", Some(SCOPE_A), Sensitivity::Secret),
                (
                    "contradicted-with-artifact",
                    Some(SCOPE_A),
                    Sensitivity::Normal,
                ),
                ("automatic", Some(SCOPE_A), Sensitivity::Normal),
            ] {
                envelope.insert_decision(decision(object, scope_id, sensitivity))?;
                if object == "automatic" {
                    envelope.insert_admission_observation_for_test(
                        "observation-automatic",
                        "code_present",
                        DOMAIN,
                        "repo",
                        "src/automatic",
                        1,
                    )?;
                    let mut request = admission_from(
                        object,
                        EventKind::CodeObserved,
                        SourceClass::TrustedLocalCode,
                        TaintClass::CurrentCode,
                    );
                    request.event.trigger_object_id = Some("observation-automatic".to_string());
                    envelope.record_admission(request)?;
                } else if !matches!(object, "unadmitted" | "secret-unadmitted") {
                    let kind = if object.starts_with("contradicted") {
                        EventKind::Contradict
                    } else {
                        EventKind::Other
                    };
                    envelope.record_admission(admission(object, kind))?;
                }
            }
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("mutate"), |envelope| {
            envelope.retire_decision("retired")?;
            envelope.retire_decision("retired-other-project")?;
            let mut replacement = decision("replacement", Some(SCOPE_A), Sensitivity::Normal);
            replacement.source_id = "src/replaced".to_string();
            replacement.source_revision = 2;
            envelope.supersede_decision("replaced", replacement)?;
            envelope.record_admission(admission("replacement", EventKind::Other))?;
            Ok(String::new())
        })
        .unwrap();
    let sensitive_artifact = store
        .ingest_artifact(artifact(
            "sensitive",
            b"sensitive bytes",
            Sensitivity::Sensitive,
        ))
        .unwrap()
        .digest;
    let secret_artifact = store
        .ingest_artifact(artifact("secret", b"secret bytes", Sensitivity::Secret))
        .unwrap()
        .digest;
    Fixture {
        _root: root,
        store,
        sensitive_artifact,
        secret_artifact,
    }
}
