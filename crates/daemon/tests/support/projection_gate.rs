//! Gates for tests: an evaluator whose evidence passes every gate under the test identity, and a gate that admits every hook, for tests whose subject is the work behind the gate rather than the gate itself.

use std::collections::BTreeMap;
use std::sync::Arc;

use daemon::coverage::ProjectionCoverage;
use daemon::projection_gates::{
    APPROVED_OBSERVERS, CAPABILITIES, CapabilityDisposition, CapabilityEvidence, Evidence,
    EvidenceEvaluator, HARNESSES, HarnessRun, HookGate, InvalidationIdentity, ProjectionHook,
    REQUIRED_LIMITS, ResourceEvidence, RuntimeManifest,
};
use kernel::EgressSnapshot;
use kernel::source_identity::OccurrenceClass;
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration, dense_eligible};
use retrieval::coverage::{ClassCoverage, CoverageReport, DenseDisposition};

pub const POLICY: &str = "source-policy.v1";
pub const MODEL: &str = "tiny-test-model";
pub const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
pub const CONTRACT: &str = "search-projection-identity-v2";
pub const LIMITS: &str = "limits.v1";

pub fn identity(kernel_incarnation_id: &str, vector_dimension: u32) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: CONTRACT.to_string(),
        limit_manifest_protocol_version: LIMITS.to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension,
        generation_epoch: 1,
    }
}

/// The coverage report an empty projection under `identity` observes at `kernel_tip`.
pub fn empty_coverage(identity: &ProjectionIdentity, kernel_tip: i64) -> ProjectionCoverage {
    ProjectionCoverage {
        report: CoverageReport {
            identity: identity.clone(),
            checkpoint: ProjectionCheckpoint {
                snapshot_commit_seq: kernel_tip,
                checkpoint_commit_seq: kernel_tip,
                hold_id: "test-hold".to_owned(),
            },
            generation: VectorGeneration {
                generation_id: "test-generation".to_owned(),
                embedding_model: identity.embedding_model.clone(),
                tokenizer_fingerprint: identity.tokenizer_fingerprint.clone(),
                vector_dimension: identity.vector_dimension,
                generation_epoch: identity.generation_epoch,
            },
            classes: OccurrenceClass::ALL
                .into_iter()
                .map(|class| ClassCoverage {
                    class,
                    dense: if dense_eligible(class) {
                        DenseDisposition::Required
                    } else {
                        DenseDisposition::LexicalOnly
                    },
                    lexical: 0,
                    dense_required: 0,
                    valid_vectors: 0,
                    missing: 0,
                    pending: 0,
                    missing_without_pending: 0,
                    tombstoned: 0,
                })
                .collect(),
        },
        kernel_snapshot: EgressSnapshot {
            tip: kernel_tip,
            classification_generation: Some(0),
        },
        exclusions: Vec::new(),
    }
}

/// An evaluator whose manifest enables `enabled` and whose evidence passes every gate under `identity`: every limit at its maximum, an approved observer at zero bytes, every required capability proved and the optional one proved off, and both harnesses passed under the identity.
pub fn passing_evaluator(
    identity: &ProjectionIdentity,
    kernel_tip: i64,
    enabled: &[ProjectionHook],
) -> EvidenceEvaluator {
    let current = InvalidationIdentity::from(identity);
    let capabilities: BTreeMap<(String, String), CapabilityEvidence> = HARNESSES
        .into_iter()
        .flat_map(|harness| {
            CAPABILITIES.into_iter().map(move |capability| {
                let proved = match capability.disposition {
                    CapabilityDisposition::Required => CapabilityEvidence::Supported,
                    CapabilityDisposition::OptionalDisabled => CapabilityEvidence::Unsupported,
                };
                ((harness.to_owned(), capability.name.to_owned()), proved)
            })
        })
        .collect();
    EvidenceEvaluator {
        manifest: RuntimeManifest {
            protocol_version: identity.limit_manifest_protocol_version.clone(),
            identity: current.clone(),
            limits: REQUIRED_LIMITS
                .into_iter()
                .map(|name| (name.to_owned(), u64::MAX))
                .collect(),
            enabled: enabled.iter().map(|hook| (*hook, true)).collect(),
        },
        current: current.clone(),
        evidence: Evidence {
            identity: current.clone(),
            coverage: Some(empty_coverage(identity, kernel_tip)),
            resource: Some(ResourceEvidence {
                observer: APPROVED_OBSERVERS[0].to_owned(),
                decoded_heap_high_water_bytes: 0,
            }),
            capabilities,
            harness_runs: HARNESSES
                .into_iter()
                .map(|harness| {
                    (
                        harness.to_owned(),
                        HarnessRun::Passed {
                            identity: current.clone(),
                        },
                    )
                })
                .collect(),
        },
    }
}

/// A gate that admits every hook under the test identity.
pub fn open_gate() -> Arc<HookGate> {
    let gate = HookGate::closed();
    gate.install(passing_evaluator(
        &identity("test-incarnation", 8),
        0,
        &ProjectionHook::ALL,
    ));
    Arc::new(gate)
}
