#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    ArmRates, Attestation, BinaryDigest, BuildRecord, ClaimBoundary, ComponentVersions,
    Construction, Cut, CutOutcome, CutReceipt, ExecutionMode, Ingestion, MANIFEST_SCHEMA, Manifest,
    MemoryReviewerModelCalls, ObservationSchema, Reachability, RepositorySpec, ResourceLimits,
    Rule, RunIdentity, RunStatus, SemanticTrace, SessionSpec, TokenizerProfile, WorldConfig,
    eval_run_id,
};
use serde_json::json;

pub const OBSERVATION_TYPE: &str = "surface1_hint_decision";
pub const WORLD_SEED: u64 = 0x5EED_0000_0000_0001;
pub const WORLD_EPOCH_MS: i64 = 1_700_000_000_000;

/// Two sessions and one repository. Session 1 fires a correction and an
/// invalidation on the same slot; four renames over three paths force a chain.
pub fn world_config() -> WorldConfig {
    WorldConfig {
        sessions: vec![
            SessionSpec {
                messages: 6,
                tool_span_every: 2,
                correction_every: 3,
                invalidation_every: 5,
            },
            SessionSpec {
                messages: 5,
                tool_span_every: 3,
                correction_every: 2,
                invalidation_every: 2,
            },
        ],
        repositories: vec![RepositorySpec {
            commits: 8,
            rename_every: 2,
        }],
        epoch_ms: WORLD_EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 64,
        planted: Vec::new(),
    }
}

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
        execution_mode: ExecutionMode::Generate,
        failure_class_table_digest: eval_core::FAILURE_CLASS_TABLE_DIGEST.to_string(),
        ingestion: Ingestion::AdapterIngestedNoProductionCaller,
        memory_reviewer_model_calls: MemoryReviewerModelCalls::Excluded,
        analysis_family_digest: None,
        recency_baseline: None,
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

/// The shrink fixtures: one aged world from `world_config`, a natural-fresh
/// history under another seed, two tasks, two inert kill episodes, and the
/// planted commit oracle.
pub mod shrink {
    use std::collections::BTreeSet;

    use eval_core::{
        APPLICATION_CRASH, Cut, Destination, EvaluatedSurface, EventId, EventLog, FailureClass,
        FailurePredicate, FaultAction, FaultEpisode, FaultScope, KillLabel, MAX_VALID_TIME_MS,
        Mode, Oracle, Query, ReplayOutcome, ReplayRequest, RepositorySpec, Scenario, Sensitivity,
        ServedClass, SessionSpec, StoreFamily, TEST_BINARY_CHILD, Task, TaskRole, Visibility,
        WitnessClass, WorldConfig, reduce, serialize_spec,
    };
    use serde_json::Value;

    use super::{WORLD_EPOCH_MS, WORLD_SEED, world_config};

    pub const FRESH_SEED: u64 = WORLD_SEED ^ 0xABCD;
    pub const PROFILE: &str = "profile-digest";
    pub const CUT: Cut = Cut::AtQuiescence;
    pub const BUDGET: u64 = 400;

    pub fn fresh_config() -> WorldConfig {
        WorldConfig {
            sessions: vec![SessionSpec {
                messages: 3,
                tool_span_every: 2,
                correction_every: 0,
                invalidation_every: 0,
            }],
            repositories: vec![RepositorySpec {
                commits: 2,
                rename_every: 0,
            }],
            epoch_ms: WORLD_EPOCH_MS,
            tick_ms: 1_000,
            max_events_per_log: 64,
            planted: Vec::new(),
        }
    }

    pub fn generate(seed: u64, config: &WorldConfig) -> EventLog {
        eval_core::generate_all(seed, config, Mode::Generate)
            .unwrap()
            .log
    }

    pub fn query() -> Query {
        Query {
            valid_time_ms: MAX_VALID_TIME_MS,
            observation_time_ms: MAX_VALID_TIME_MS,
            scope: BTreeSet::from([
                "session-0".to_string(),
                "session-1".to_string(),
                "repository-0".to_string(),
            ]),
            destination: Destination::Local,
            served: Some(ServedClass {
                sensitivity: Sensitivity::Normal,
                visibility: Visibility::Labeled,
                auto_inject: Visibility::Hidden,
                auto_search: Visibility::Hidden,
            }),
            registry_sensitivity: Sensitivity::Normal,
            max_events_per_log: 64,
        }
    }

    pub fn task(name: &str, role: TaskRole, evidence: &str) -> Task {
        Task {
            id: name.to_string(),
            role,
            query: query(),
            evidence: BTreeSet::from([EventId(evidence.to_string())]),
        }
    }

    pub fn episode(id: &str) -> FaultEpisode {
        let action = FaultAction::ProcessKill {
            cut: "local_staged".to_string(),
        };
        FaultEpisode {
            id: id.to_string(),
            trigger_step: 3,
            scope: FaultScope {
                store: StoreFamily::SearchProjection,
                operation: "acknowledge".to_string(),
            },
            heal: action.heal(),
            action,
            layer_contract: "search_catchup::EpisodeFault".to_string(),
            kill: Some(KillLabel {
                crash_model: APPLICATION_CRASH.to_string(),
                page_cache_intact: true,
                killed_process: TEST_BINARY_CHILD.to_string(),
            }),
        }
    }

    pub fn scenario() -> Scenario {
        Scenario {
            surface: EvaluatedSurface::Surface1,
            recency_bound: None,
            aged: generate(WORLD_SEED, &world_config()),
            natural_fresh: generate(FRESH_SEED, &fresh_config()),
            tasks: vec![
                task(
                    "early-commit",
                    TaskRole::Falsification,
                    "repository:repository-0:0",
                ),
                task(
                    "last-rename",
                    TaskRole::PositiveControl,
                    "repository:repository-0:11",
                ),
            ],
            episodes: vec![episode("kill-1"), episode("kill-2")],
        }
    }

    pub fn fixture() -> Value {
        serialize_spec()
    }

    pub fn oracle() -> Oracle {
        Oracle::RequiredCommits {
            failing_at: 3,
            slipping_at: 6,
        }
    }

    pub fn predicate(class: FailureClass) -> FailurePredicate {
        FailurePredicate {
            oracle: oracle(),
            checkpoint: CUT,
            profile_digest: PROFILE.to_string(),
            witness_class: WitnessClass::Failure { class },
        }
    }

    /// The in-process replay: the planted oracle over the compiled candidate and
    /// the aged truth reduced at the first task's cut.
    pub fn evaluate(request: ReplayRequest<'_>) -> ReplayOutcome {
        let truth = reduce(
            &request.set.aged,
            &fixture(),
            &request.set.pairs[0].task.query,
        )
        .unwrap();
        request.oracle.evaluate(
            request.set,
            &truth,
            request.checkpoint,
            request.profile_digest,
        )
    }
}
