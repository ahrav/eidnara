//! Gates for tests: an evaluator whose evidence passes every gate under the test identity, and a gate that admits every hook, for tests whose subject is the work behind the gate rather than the gate itself.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use daemon::coverage::ProjectionCoverage;
use daemon::projection_admission::{ADMISSION_DIR, EVIDENCE_RECORD, MANIFEST_RECORD};
use daemon::projection_gates::{
    APPROVED_OBSERVERS, CAPABILITIES, COMPRESSION_CRITERIA, CapabilityDisposition,
    CapabilityEvidence, CompressionBinding, CompressionEvidence, CompressionRecord, Evidence,
    EvidenceEvaluator, FullPathTrace, HARNESSES, HarnessRun, HookGate, InvalidationIdentity,
    Outcome, ProjectionHook, REQUIRED_LIMITS, ResourceEvidence, RuntimeManifest, TRACE_STAGES,
    TraceKind, VECTOR_LIMITS,
};
use retrieval::ProjectionIdentity;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

pub const POLICY: &str = "source-policy.v1";
pub const MODEL: &str = "tiny-test-model";
pub const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
pub const CONTRACT: &str = "search-projection-identity-v3";
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
        analysis_identity: retrieval::lexical::AnalysisIdentity::current()
            .as_str()
            .to_string(),
        vector_dimension,
        generation_epoch: 1,
    }
}

/// The coverage report an empty projection under `identity` observes at `kernel_tip`.
pub fn empty_coverage(identity: &ProjectionIdentity, kernel_tip: i64) -> ProjectionCoverage {
    ProjectionCoverage::unregistered(identity, kernel_tip)
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
                .chain(VECTOR_LIMITS)
                .map(|name| (name.to_owned(), u64::MAX))
                .collect(),
            enabled: enabled.iter().map(|hook| (*hook, true)).collect(),
            compressed_activation: true,
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
            compression: CompressionRecord::Campaign(Box::new(passing_compression(&current))),
        },
        binding: Some(test_binding()),
    }
}

/// The binding the test daemon runs under; the passing compression evidence names the same one.
pub fn test_binding() -> CompressionBinding {
    CompressionBinding {
        build: "eidnara-test-build".to_owned(),
        corpus_sha256: "c".repeat(64),
        quantizer_recipe: "scalar-int8-symmetric.v1".to_owned(),
        hardware: "test-hardware".to_owned(),
        harnesses: HARNESSES
            .into_iter()
            .map(|harness| (harness.to_owned(), "test".to_owned()))
            .collect(),
    }
}

/// A compression campaign that passed every criterion under `current`, the test binding, and unbounded vector limits, with a real full-path trace from both harnesses.
pub fn passing_compression(current: &InvalidationIdentity) -> CompressionEvidence {
    CompressionEvidence {
        identity: current.clone(),
        binding: test_binding(),
        limits: VECTOR_LIMITS
            .into_iter()
            .map(|name| (name.to_owned(), u64::MAX))
            .collect(),
        criteria: COMPRESSION_CRITERIA
            .into_iter()
            .map(|criterion| (criterion.to_owned(), Outcome::Passed))
            .collect(),
        traces: HARNESSES
            .into_iter()
            .map(|harness| {
                (
                    harness.to_owned(),
                    FullPathTrace {
                        kind: TraceKind::Real,
                        stages: TRACE_STAGES
                            .into_iter()
                            .map(str::to_owned)
                            .collect::<BTreeSet<_>>(),
                    },
                )
            })
            .collect(),
        revoked: false,
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

pub const LAG_LIMIT: u64 = 4;
pub const LIMIT: u64 = 1_000_000;
pub const HEAP_BYTES: u64 = 1024;

/// The fixture manifest's value for `name`: a short lag, a disk bound wide enough for a staged seed and its capture, and `LIMIT` for the rest.
pub fn fixture_limit(name: &str) -> u64 {
    match name {
        "catchup_lag_commits" => LAG_LIMIT,
        "capture_disk_bytes" => 64 << 20,
        _ => LIMIT,
    }
}

pub fn manifest_json(identity: &ProjectionIdentity, enabled: &[ProjectionHook]) -> Value {
    manifest_json_with(identity, enabled, &[])
}

/// The fixture manifest with `overrides` replacing the named limits.
pub fn manifest_json_with(
    identity: &ProjectionIdentity,
    enabled: &[ProjectionHook],
    overrides: &[(&str, u64)],
) -> Value {
    let limits: serde_json::Map<String, Value> = REQUIRED_LIMITS
        .into_iter()
        .map(|name| {
            let value = overrides
                .iter()
                .find(|(overridden, _)| *overridden == name)
                .map_or_else(|| fixture_limit(name), |(_, value)| *value);
            (name.to_owned(), json!(value))
        })
        .collect();
    let hooks: serde_json::Map<String, Value> = ProjectionHook::ALL
        .iter()
        .map(|hook| {
            (
                hook.id().to_owned(),
                json!({ "enabled": enabled.contains(hook) }),
            )
        })
        .collect();
    json!({
        "protocol_version": identity.limit_manifest_protocol_version,
        "invalidation_identity": invalidation_json(identity),
        "limits": limits,
        "hooks": hooks,
    })
}

pub fn invalidation_json(identity: &ProjectionIdentity) -> Value {
    json!({
        "schema_version": identity.schema_version,
        "tokenizer_fingerprint": identity.tokenizer_fingerprint,
        "analysis_identity": identity.analysis_identity,
        "embedding_model": identity.embedding_model,
        "projection_policy_version": identity.projection_policy_version,
        "identity_contract_version": identity.identity_contract_version,
        "limit_manifest_protocol_version": identity.limit_manifest_protocol_version,
        "vector_dimension": identity.vector_dimension,
        "generation_epoch": identity.generation_epoch,
    })
}

/// A campaign record whose every dimension passes under `identity`.
pub fn campaign_json(identity: &ProjectionIdentity) -> Value {
    let proved: serde_json::Map<String, Value> = CAPABILITIES
        .iter()
        .map(|capability| {
            let outcome = match capability.disposition {
                CapabilityDisposition::Required => "supported",
                CapabilityDisposition::OptionalDisabled => "unsupported",
            };
            (capability.name.to_owned(), json!(outcome))
        })
        .collect();
    let capabilities: serde_json::Map<String, Value> = HARNESSES
        .iter()
        .map(|harness| ((*harness).to_owned(), Value::Object(proved.clone())))
        .collect();
    let harness_runs: serde_json::Map<String, Value> = HARNESSES
        .iter()
        .map(|harness| ((*harness).to_owned(), passed_run_json(identity)))
        .collect();
    json!({
        "invalidation_identity": invalidation_json(identity),
        "resource": {
            "observer": APPROVED_OBSERVERS[0],
            "decoded_heap_high_water_bytes": HEAP_BYTES,
        },
        "capabilities": capabilities,
        "harness_runs": harness_runs,
    })
}

pub fn write_record(home: &Path, record: &str, bytes: &[u8]) {
    let dir = home.join(ADMISSION_DIR);
    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&dir) {
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    }
    let path = dir.join(record);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn write_records(home: &Path, manifest: &Value, campaign: &Value) {
    write_record(
        home,
        MANIFEST_RECORD,
        &serde_json::to_vec_pretty(manifest).unwrap(),
    );
    write_record(
        home,
        EVIDENCE_RECORD,
        &serde_json::to_vec_pretty(campaign).unwrap(),
    );
}

/// A passed harness run recorded under `identity`.
pub fn passed_run_json(identity: &ProjectionIdentity) -> Value {
    json!({
        "outcome": "passed",
        "invalidation_identity": invalidation_json(identity),
    })
}
