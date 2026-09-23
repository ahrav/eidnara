//! The witness package: the original failure, the minimized scenario, the
//! compact recipe, the claim boundary, residue drift, and the limits that
//! apply before a byte is published.

mod support;

use std::collections::BTreeSet;

use context_core::redaction::{RedactionErrorKind, Redactor};
use eval_core::{
    APPLICATION_CRASH, ClaimBoundary, Cut, Destination, EvaluatedSurface, EventId, EventLog,
    FailureClass, FailurePredicate, FaultAction, FaultEpisode, FaultScope, Generation, KillLabel,
    MAX_VALID_TIME_MS, Mode, MultiplicityRecipe, Oracle, OriginalFailure, Query, ReplayOutcome,
    ReplayRequest, RepositorySpec, Scenario, Sensitivity, ServedClass, SessionSpec, Slice,
    StoreFamily, TEST_BINARY_CHILD, Task, TaskRole, Visibility, WITNESS_SCHEMA, WitnessClass,
    WitnessError, WitnessPackage, WorldConfig, multiplicities, parse_witness, reduce,
    serialize_spec, shrink,
};
use serde_json::Value;
use support::{WORLD_EPOCH_MS, WORLD_SEED, world_config};

const FRESH_SEED: u64 = WORLD_SEED ^ 0xABCD;
const PROFILE: &str = "profile-digest";
const CUT: Cut = Cut::AtQuiescence;
const BUDGET: u64 = 400;

fn fresh_config() -> WorldConfig {
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

fn generate(seed: u64, config: &WorldConfig) -> EventLog {
    eval_core::generate_all(seed, config, Mode::Generate)
        .unwrap()
        .log
}

fn query() -> Query {
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

fn task(name: &str, role: TaskRole, evidence: &str) -> Task {
    Task {
        id: name.to_string(),
        role,
        query: query(),
        evidence: BTreeSet::from([EventId(evidence.to_string())]),
    }
}

fn episode(id: &str) -> FaultEpisode {
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

fn scenario() -> Scenario {
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

fn fixture() -> Value {
    serialize_spec()
}

fn oracle() -> Oracle {
    Oracle::RequiredCommits {
        failing_at: 3,
        slipping_at: 6,
    }
}

fn predicate(class: FailureClass) -> FailurePredicate {
    FailurePredicate {
        oracle: oracle().name().to_string(),
        checkpoint: CUT,
        profile_digest: PROFILE.to_string(),
        witness_class: WitnessClass::Failure { class },
    }
}

/// The in-process replay: the planted oracle over the compiled candidate and
/// the aged truth reduced at the first task's cut.
fn evaluate(request: ReplayRequest<'_>) -> ReplayOutcome {
    let truth = reduce(
        &request.set.aged,
        &fixture(),
        &request.set.pairs[0].task.query,
    )
    .unwrap();
    oracle().evaluate(
        request.set,
        &truth,
        request.checkpoint,
        request.profile_digest,
    )
}

fn package() -> WitnessPackage {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    let world = eval_core::generate_all(WORLD_SEED, &world_config(), Mode::Generate).unwrap();
    WitnessPackage {
        schema: WITNESS_SCHEMA.to_string(),
        original: OriginalFailure {
            eval_run_id: "ab".repeat(32),
            tape: world.tape,
            trace_digest: "cd".repeat(32),
            causal_trace: original.aged.causal_edges.clone(),
            predicate: expected,
            coverage: BTreeSet::from(["ing_observation_time_inert".to_string()]),
        },
        slice: Slice::Cassette,
        replayable: true,
        residue: support::manifest().residue,
        recipe: Some(MultiplicityRecipe {
            aged: Generation {
                config: world_config(),
                root_seed: WORLD_SEED,
            },
            natural_fresh: Generation {
                config: fresh_config(),
                root_seed: FRESH_SEED,
            },
            deleted: report.deleted.clone(),
            multiplicities: multiplicities(&minimized),
        }),
        minimized,
        shrink: report,
        claim_boundary: ClaimBoundary::pinned(),
    }
}

#[test]
fn the_package_round_trips_and_carries_the_recipe_for_a_count_triggered_failure() {
    let package = package();
    let value = package.serialize(None, u64::MAX).unwrap();
    assert_eq!(parse_witness(&value).unwrap(), package);
    assert_eq!(package.recipe.as_ref().unwrap().multiplicities["commit"], 6);

    let mut without_recipe = package.clone();
    without_recipe.recipe = None;
    assert_eq!(without_recipe.validate(), Err(WitnessError::RecipeRequired));

    let mut wrong_seed = package.clone();
    wrong_seed.recipe.as_mut().unwrap().aged.root_seed ^= 1;
    assert_eq!(
        wrong_seed.validate(),
        Err(WitnessError::RecipeDisagrees { world: "aged" })
    );

    let mut tampered = value.clone();
    tampered["extra"] = Value::Bool(true);
    assert!(matches!(
        parse_witness(&tampered),
        Err(WitnessError::Shape(_))
    ));
}

#[test]
fn live_model_evidence_is_never_relabelled_replayable() {
    let mut witness = package();
    witness.slice = Slice::Live;
    assert_eq!(
        witness.validate(),
        Err(WitnessError::LiveRelabelledReplayable)
    );
    witness.replayable = false;
    witness.validate().unwrap();
}

#[test]
fn the_serializer_requires_the_verbatim_claim_boundary_and_rejects_forbidden_claims() {
    let mut witness = package();
    witness.claim_boundary.exclusions.pop();
    assert_eq!(witness.validate(), Err(WitnessError::ClaimBoundaryMismatch));

    let mut claims = package();
    claims.original.predicate.oracle = "proves live-model quality".to_string();
    claims.shrink.predicate.oracle = claims.original.predicate.oracle.clone();
    assert_eq!(
        claims.validate(),
        Err(WitnessError::ForbiddenClaim {
            path: "/original/predicate/oracle".to_string(),
            phrase: "live-model quality".to_string(),
        })
    );
    let mut disagrees = package();
    disagrees.shrink.predicate.checkpoint = Cut::EndOfRun;
    assert_eq!(disagrees.validate(), Err(WitnessError::PredicateDisagrees));
}

#[test]
fn residue_drift_refuses_and_limits_apply_before_publication() {
    let package = package();
    let mut drifted = package.residue.clone();
    let moved = drifted.pop_first().unwrap();
    package.check_residue(&package.residue).unwrap();
    let refused = package.check_residue(&drifted).unwrap_err();
    assert_eq!(
        refused,
        WitnessError::ResidueDrift {
            missing: BTreeSet::from([moved]),
            unexpected: BTreeSet::new(),
        }
    );
    assert!(matches!(
        package.serialize(None, 16),
        Err(WitnessError::TooLarge { bound: 16, .. })
    ));
    let redactor = Redactor::new().unwrap();
    package.serialize(Some(&redactor), u64::MAX).unwrap();
    let mut leaking = package.clone();
    leaking.original.coverage.insert(
        "Authorization: Bearer sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcd".to_string(),
    );
    assert_eq!(
        leaking.serialize(Some(&redactor), u64::MAX).err(),
        Some(WitnessError::RedactionRefused(
            RedactionErrorKind::SecretDetected
        ))
    );
}
