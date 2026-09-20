#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    ArmRates, Attestation, BinaryDigest, BuildRecord, ClaimBoundary, ComponentVersions,
    Construction, Cut, CutOutcome, CutReceipt, MANIFEST_SCHEMA, Manifest, ObservationSchema,
    Reachability, ResourceLimits, Rule, RunIdentity, RunStatus, SemanticTrace, TokenizerProfile,
    eval_run_id,
};
use serde_json::json;

pub const OBSERVATION_TYPE: &str = "surface1_hint_decision";

pub fn build() -> BuildRecord {
    BuildRecord {
        code_sha: "3f".repeat(20),
        dirty: false,
        lockfile_digest: "ab".repeat(32),
        rustc_version: "rustc 1.98.0".to_string(),
        features: BTreeSet::from(["test-support".to_string()]),
        target_triple: "x86_64-unknown-linux-gnu".to_string(),
        binary_digest: BinaryDigest::Present {
            sha256: "cd".repeat(32),
        },
    }
}

pub fn identity() -> RunIdentity {
    RunIdentity {
        build: build(),
        simulator_version: "sim-1".to_string(),
        config: json!({"auto_search": true, "surface": 1}),
        scenario: json!({"world": "falsification-pair-1"}),
        root_seed: 0xDEAD_BEEF_CAFE_F00D,
        random_schema_version: "rng-1".to_string(),
        generator_version: "gen-1".to_string(),
        eligibility_spec_digest: "ef".repeat(32),
        linearization_rule_version: "lin-1".to_string(),
    }
}

/// A schema exercising every rule, with every allowlisted clock field under `Keep`.
pub fn observation_schema() -> ObservationSchema {
    ObservationSchema::new(
        OBSERVATION_TYPE,
        [
            ("occurrence_id", Rule::Keep),
            ("hint_text", Rule::Keep),
            ("now_ms", Rule::Keep),
            ("observed_at_ms", Rule::Keep),
            ("valid_time_ms", Rule::Keep),
            ("decided_at_ms", Rule::Drop),
            ("run_id", Rule::Drop),
            ("hold_expires_at", Rule::Presence),
            ("database_incarnation_id", Rule::Relative),
        ],
    )
    .unwrap()
}

pub fn observation(sequence: u64, incarnation: &str, hold: Option<i64>) -> serde_json::Value {
    json!({
        "occurrence_id": format!("occ-{sequence}"),
        "hint_text": format!("fragment {sequence}"),
        "now_ms": 1_700_000_000_000_i64 + sequence as i64,
        "observed_at_ms": 1_700_000_000_500_i64,
        "valid_time_ms": 1_600_000_000_000_i64,
        "decided_at_ms": 4_102_444_800_000_i64 + sequence as i64,
        "run_id": format!("model_execution-7-{sequence}"),
        "hold_expires_at": hold,
        "database_incarnation_id": incarnation,
    })
}

pub fn trace() -> SemanticTrace {
    let mut trace = SemanticTrace::new([observation_schema()]).unwrap();
    for sequence in 0..3 {
        trace
            .record(OBSERVATION_TYPE, &observation(sequence, "inc-a", Some(5)))
            .unwrap();
    }
    trace
}

pub fn limits(scale: u64) -> ResourceLimits {
    ResourceLimits {
        elapsed_ms: 60_000 * scale,
        store_bytes: 1 << 30,
        cassette_bytes: 1 << 26,
        artifact_bytes: 1 << 24,
        temp_roots: 4,
        retained_artifacts: 16,
        processes: 8,
    }
}

pub fn manifest_for(identity: RunIdentity, trace: &SemanticTrace) -> Manifest {
    let residue: BTreeSet<_> = Manifest::field_schema()
        .residue()
        .chain(trace.residue())
        .collect();
    Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&identity).unwrap(),
        run_identity: identity.clone(),
        start_ms: 1_700_000_000_000,
        end_ms: 1_700_000_060_000,
        status: RunStatus::Completed,
        error: None,
        sample_ids: vec!["pair-1".to_string(), "pair-2".to_string()],
        sample_order: vec!["pair-2".to_string(), "pair-1".to_string()],
        sample_epoch: 1,
        retry_lineage: Vec::new(),
        result_digest: "12".repeat(32),
        witness_digest: "34".repeat(32),
        attestation: Attestation::None,
        tokenizer_profile: TokenizerProfile {
            name: "claude".to_string(),
            revision: "tiktoken-1".to_string(),
            digest: "56".repeat(32),
        },
        cut_receipts: vec![CutReceipt {
            cut: Cut::AtQuiescence,
            outcome: CutOutcome::Reached,
        }],
        residue,
        construction: Construction::HandBuilt,
        reachability: Reachability::DefaultProduction,
        claim_boundary: ClaimBoundary::pinned(),
        component_versions: ComponentVersions {
            generator: identity.generator_version,
            event_schema: "events-1".to_string(),
            reducer: "reducer-1".to_string(),
            oracles: "oracles-1".to_string(),
            execution_image: "image-1".to_string(),
            task_corpus: "corpus-1".to_string(),
            judge: "judge-1".to_string(),
        },
        envelope_bounds: limits(10),
        envelope_peaks: limits(1),
        arm_rates: BTreeMap::from([(
            "fresh".to_string(),
            ArmRates {
                miss_rate: "0".to_string(),
                refusal_rate: "0.25".to_string(),
            },
        )]),
    }
}

pub fn manifest() -> Manifest {
    manifest_for(identity(), &trace())
}
