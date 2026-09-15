//! A real-store fixture for the claim causality tests: a domain, admitted
//! decisions, exact-retained artifacts, and causality records written through
//! the public API.

#![allow(dead_code)]

use std::num::NonZeroU64;

use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactIngestRequest, CausalClass, CausalEvidence,
    CausalReading, ClaimCausalityError, ClaimCausalityRequest, CommitIntent, DecisionPayload,
    DecisionSpec, DomainSpec, EventKind, KernelStore, ParentReference, ProviderEgress,
    RepositoryProvenance, Sensitivity, SourceClass, TaintClass,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

pub const DOMAIN: &str = "domain";
pub const PRODUCER: &str = "kernel-claim-causality-test";

pub fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: PRODUCER.to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

pub const MAX_DETAIL_BYTES: NonZeroU64 = NonZeroU64::new(1 << 16).unwrap();

pub fn decision(index: i64, revision: i64) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{index}"),
        object_id: format!("decision-object-{index}"),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: None,
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("decision {index}"),
            rationale: format!("because {index}"),
        },
        source_kind: "fixture".to_string(),
        source_id: "claim-lineage".to_string(),
        source_revision: revision,
        sensitivity: Sensitivity::Normal,
    }
}

pub fn admission(object_id: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_string()),
        source_class: Some(SourceClass::TrustedLocalCode),
        taint_class: Some(TaintClass::CurrentCode),
        event: AdmissionEvent {
            kind: EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".to_string(),
        },
    }
}

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub store: KernelStore,
}

impl Fixture {
    pub fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = KernelStore::open(root.path()).unwrap();
        store
            .commit(intent("domain"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                Ok(String::new())
            })
            .unwrap();
        Self { root, store }
    }

    pub fn reopen(self) -> Self {
        let Fixture { root, store } = self;
        drop(store);
        let store = KernelStore::open(root.path()).unwrap();
        Self { root, store }
    }

    pub fn retain(&self, key: &str, text: &str) -> (String, String) {
        let handle = self
            .store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "tool_output".to_string(),
                source_id: format!("native/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: Some(RepositoryProvenance {
                    repository_id: "repo".to_string(),
                    revision: "abc123".to_string(),
                }),
            })
            .unwrap();
        (handle.evidence_id, handle.digest)
    }

    /// Inserts and admits `decision(index, revision)`, returning the admission
    /// the writer reported and the commit it landed in.
    pub fn admit_decision(&self, index: i64, revision: i64) -> (kernel::AdmissionDecision, i64) {
        let mut reported = None;
        let receipt = self
            .store
            .commit(intent(&format!("decision-{index}")), |envelope| {
                envelope.insert_decision(decision(index, revision))?;
                reported = Some(
                    envelope.record_admission(admission(&format!("decision-object-{index}")))?,
                );
                Ok(String::new())
            })
            .unwrap();
        (reported.unwrap(), receipt.commit_seq)
    }

    pub fn record(
        &self,
        key: &str,
        request: ClaimCausalityRequest<'_>,
    ) -> Result<kernel::ClaimCausalityOutcome, ClaimCausalityError> {
        let mut outcome = None;
        let mut refusal = None;
        let committed = self.store.commit(intent(key), |envelope| {
            match envelope.record_claim_causality(&request) {
                Ok(recorded) => outcome = Some(recorded),
                Err(error) => refusal = Some(error),
            }
            Ok(String::new())
        });
        match (committed, refusal) {
            (Ok(_), None) => Ok(outcome.unwrap()),
            (Err(_), Some(refusal)) => Err(refusal),
            (committed, refusal) => panic!("unexpected outcome {committed:?} {refusal:?}"),
        }
    }

    pub fn reading(&self, object_id: &str, as_of: i64) -> CausalReading {
        self.store
            .causal_class_as_of(object_id, as_of, MAX_DETAIL_BYTES)
            .unwrap()
    }

    pub fn class(&self, object_id: &str, as_of: i64) -> CausalClass {
        self.reading(object_id, as_of).class
    }

    /// Ingests `text` through the redacting path, so a payload holding a
    /// recognized secret is retained with a rewrite and is not exact.
    pub fn retain_redacted(&self, key: &str, text: &str) -> (String, String) {
        let handle = self
            .store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "tool_output".to_string(),
                source_id: format!("native/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: Some(RepositoryProvenance {
                    repository_id: "repo".to_string(),
                    revision: "abc123".to_string(),
                }),
            })
            .unwrap();
        (handle.evidence_id, handle.digest)
    }

    pub fn tip(&self) -> i64 {
        self.store.tip().unwrap()
    }

    pub fn sql(&self, statement: &str) {
        let connection = Connection::open(self.root.path().join("kernel.sqlite")).unwrap();
        connection.execute_batch(statement).unwrap();
    }
}

pub fn direct(evidence_id: &str, digest: &str) -> CausalEvidence {
    CausalEvidence::DirectObservation {
        acquisition_evidence_id: evidence_id.to_string(),
        artifact_digest: digest.to_string(),
    }
}

pub fn derived(parents: &[(&str, i64)]) -> CausalEvidence {
    CausalEvidence::DerivedReinjection {
        parents: parents
            .iter()
            .map(|(object_id, revision)| ParentReference {
                object_id: object_id.to_string(),
                revision: *revision,
            })
            .collect(),
    }
}

pub fn request<'a>(
    subject: &'a str,
    revision: i64,
    evidence: CausalEvidence,
) -> ClaimCausalityRequest<'a> {
    ClaimCausalityRequest {
        subject_object_id: subject,
        subject_revision: revision,
        evidence,
        observed_at: 7,
    }
}
