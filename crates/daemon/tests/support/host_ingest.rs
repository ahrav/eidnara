use kernel::{
    ArtifactHandle, ArtifactIngestRequest, CommitIntent, KernelStore, ProviderEgress,
    RepositoryProvenance, Sensitivity,
};

pub const REVISION: &str = "0000000000000000000000000000000000000000";

pub struct HostEvidence<'a> {
    pub intent: CommitIntent,
    pub payload: &'a str,
    pub evidence_id: String,
    pub object_id: String,
    pub domain_id: &'a str,
    pub source_kind: &'a str,
    pub source_id: String,
    pub repository_id: Option<&'a str>,
}

pub fn ingest(kernel: &KernelStore, evidence: HostEvidence<'_>) -> ArtifactHandle {
    kernel
        .ingest_artifact(ArtifactIngestRequest {
            intent: evidence.intent,
            payload: evidence.payload.as_bytes().to_vec(),
            evidence_id: evidence.evidence_id,
            object_id: evidence.object_id,
            object_kind: "evidence".to_string(),
            domain_id: evidence.domain_id.to_string(),
            source_kind: evidence.source_kind.to_string(),
            source_id: evidence.source_id,
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: evidence
                .repository_id
                .map(|repository_id| RepositoryProvenance {
                    repository_id: repository_id.to_string(),
                    revision: REVISION.to_string(),
                }),
        })
        .unwrap()
}
