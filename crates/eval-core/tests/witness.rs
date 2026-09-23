//! The witness package: the original failure, the minimized scenario, the
//! compact recipe, the claim boundary, residue drift, and the limits that
//! apply before a byte is published.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use context_core::redaction::{RedactionErrorKind, Redactor};
use eval_core::{
    CandidateVerdict, ClaimBoundary, Cut, Element, FailureClass, Generation, History, Minimality,
    Mode, MultiplicityRecipe, Oracle, OracleRefused, OriginalFailure, ShrinkReportError, Slice,
    WITNESS_SCHEMA, WitnessError, WitnessPackage, parse_witness, residue_drift, shrink,
};
use serde_json::Value;
use support::shrink::{BUDGET, FRESH_SEED, evaluate, fixture, fresh_config, predicate, scenario};
use support::{WORLD_SEED, world_config};

const ARTIFACT_BYTES: u64 = 1 << 20;

type Mutate = fn(&mut WitnessPackage);

fn redactor() -> Redactor {
    Redactor::new().unwrap()
}

fn generation(seed: u64, config: eval_core::WorldConfig) -> Generation {
    Generation {
        config,
        root_seed: seed,
    }
}

fn package() -> WitnessPackage {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    let world = eval_core::generate_all(WORLD_SEED, &world_config(), Mode::Generate).unwrap();
    let mut package = WitnessPackage {
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
        minimized,
        recipe: None,
        shrink: report,
        claim_boundary: ClaimBoundary::pinned(),
    };
    package.recipe = Some(MultiplicityRecipe {
        aged: generation(WORLD_SEED, world_config()),
        natural_fresh: generation(FRESH_SEED, fresh_config()),
        multiplicities: package.count_triggered(),
    });
    package
}

#[test]
fn the_package_round_trips_and_carries_the_recipe_for_a_count_triggered_failure() {
    let package = package();
    let (value, text) = package.serialize(&redactor(), ARTIFACT_BYTES).unwrap();
    assert_eq!(parse_witness(&value).unwrap(), package);
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
    assert_eq!(
        package.recipe.as_ref().unwrap().multiplicities,
        BTreeMap::from([("commit".to_string(), 5)]),
        "six commits remain; deleting any of the five that are not the evidence slips the class"
    );

    let mut without_recipe = package.clone();
    without_recipe.recipe = None;
    assert_eq!(without_recipe.validate(), Err(WitnessError::RecipeRequired));

    let mut wrong_seed = package.clone();
    wrong_seed.recipe.as_mut().unwrap().aged.root_seed ^= 1;
    assert_eq!(
        wrong_seed.validate(),
        Err(WitnessError::RecipeDisagrees {
            history: History::Aged
        })
    );
    let mut wrong_fresh = package.clone();
    wrong_fresh.recipe.as_mut().unwrap().natural_fresh.root_seed ^= 1;
    assert_eq!(
        wrong_fresh.validate(),
        Err(WitnessError::RecipeDisagrees {
            history: History::NaturalFresh
        })
    );
    let mut oversized = package.clone();
    oversized.recipe.as_mut().unwrap().aged.config.repositories[0].commits = 1_000_000;
    assert_eq!(
        oversized.validate(),
        Err(WitnessError::RecipeDisagrees {
            history: History::Aged
        }),
        "a declared size the minimized log cannot account for is refused before generating"
    );
    let mut wrong_counts = package.clone();
    wrong_counts
        .recipe
        .as_mut()
        .unwrap()
        .multiplicities
        .insert("message".to_string(), 2);
    assert_eq!(
        wrong_counts.validate(),
        Err(WitnessError::RecipeMultiplicitiesDisagree)
    );
    // A ledger-consistent budget claim: the run spent exactly its budget.
    let mut not_minimal = package.clone();
    not_minimal.shrink.minimality = Minimality::NotEstablished {
        reason: eval_core::NotEstablishedReason::ReplayBudgetExhausted,
    };
    not_minimal.shrink.max_replays = not_minimal.shrink.replays;
    not_minimal.recipe = None;
    not_minimal.validate().unwrap();
    let mut no_trigger = package.clone();
    no_trigger.shrink.candidates.retain(|record| {
        !matches!(
            record.verdict,
            CandidateVerdict::Slipped { .. } | CandidateVerdict::NotReproduced
        )
    });
    assert_eq!(
        no_trigger.validate(),
        Err(WitnessError::RecipeWithoutMultiplicity),
        "without a single deletion that changed the outcome, no count is the trigger"
    );

    let mut tampered = value.clone();
    tampered["extra"] = Value::Bool(true);
    assert!(matches!(
        parse_witness(&tampered),
        Err(WitnessError::Shape(_))
    ));
    let mut lossy = value.clone();
    lossy["shrink"]["deleted"].as_array_mut().unwrap().reverse();
    assert_eq!(
        parse_witness(&lossy).err(),
        Some(WitnessError::Lossy),
        "a set written out of order is not the value the type would write"
    );
}

#[test]
fn every_structural_refusal_names_its_cause() {
    let package = package();
    let cases: Vec<(Mutate, WitnessError)> = vec![
        (
            |p| p.schema = "eval-witness/v0".to_string(),
            WitnessError::SchemaMismatch {
                found: "eval-witness/v0".to_string(),
            },
        ),
        (
            |p| p.shrink.schema = "eval-shrink/v0".to_string(),
            WitnessError::ShrinkReport(ShrinkReportError::SchemaMismatch {
                found: "eval-shrink/v0".to_string(),
            }),
        ),
        (
            |p| {
                p.shrink.predicate.oracle = Oracle::RequiredCommits {
                    failing_at: 6,
                    slipping_at: 3,
                };
                p.original.predicate.oracle = p.shrink.predicate.oracle.clone();
            },
            WitnessError::ShrinkReport(ShrinkReportError::Oracle(
                OracleRefused::InvertedThresholds {
                    failing_at: 6,
                    slipping_at: 3,
                },
            )),
        ),
        (
            |p| p.original.eval_run_id = "nope".to_string(),
            WitnessError::NotHex {
                field: "eval_run_id",
            },
        ),
        (
            |p| p.original.trace_digest = "AB".repeat(32),
            WitnessError::NotHex {
                field: "trace_digest",
            },
        ),
        (
            |p| p.shrink.minimized_digest = "00".repeat(32),
            WitnessError::MinimizedDigestMismatch,
        ),
        (
            |p| p.shrink.predicate.checkpoint = Cut::EndOfRun,
            WitnessError::PredicateDisagrees,
        ),
    ];
    for (mutate, expected) in cases {
        let mut mutated = package.clone();
        mutate(&mut mutated);
        assert_eq!(mutated.validate(), Err(expected.clone()));
        assert_eq!(
            mutated.serialize(&redactor(), ARTIFACT_BYTES).err(),
            Some(expected),
            "the serializer refuses what validate refuses"
        );
    }
}

#[test]
fn one_minimality_needs_a_rejected_record_for_every_single_deletion() {
    // A ledger-consistent report that replayed only the original and claims
    // it 1-minimal: nothing was ever deleted, so no deletion was rejected.
    let mut bare = package();
    bare.minimized = scenario();
    bare.shrink.candidates.truncate(1);
    bare.shrink.minimized_digest = bare.shrink.original_digest.clone();
    bare.shrink.deleted.clear();
    bare.shrink.replays = 1;
    bare.shrink.max_replays = 1;
    bare.shrink.unknown_candidates = 0;
    bare.recipe = None;
    let element = bare.minimized.elements()[0].clone();
    assert_eq!(
        bare.validate(),
        Err(WitnessError::MinimalityUnsupported {
            element: element.clone()
        }),
        "a 1-minimal claim with no candidate records has no evidence"
    );
    assert_eq!(
        bare.serialize(&redactor(), ARTIFACT_BYTES).err(),
        Some(WitnessError::MinimalityUnsupported { element })
    );
    bare.shrink.minimality = Minimality::NotEstablished {
        reason: eval_core::NotEstablishedReason::ReplayBudgetExhausted,
    };
    bare.validate()
        .expect("a report that claims no minimality owes no rejection records");

    // A rejection record is evidence only for the scenario it names: the
    // same deletion set under a foreign digest is no record at all.
    let mut forged = package();
    let element = forged.minimized.elements()[0].clone();
    let mut deleted = forged.shrink.deleted.clone();
    deleted.insert(element.clone());
    let mut hit = 0;
    for record in &mut forged.shrink.candidates {
        if record.deleted == deleted {
            record.scenario_digest = "00".repeat(32);
            hit += 1;
        }
    }
    assert!(hit > 0, "the final pass recorded this deletion");
    assert_eq!(
        forged.validate(),
        Err(WitnessError::MinimalityUnsupported { element })
    );
}

#[test]
fn a_multiplicity_record_counts_only_under_its_own_scenario_digest() {
    let package = package();
    let counted = package.count_triggered();
    assert_eq!(counted["commit"], 5);
    // Forge the digest on one counted commit's single-deletion records: the
    // deletion set and verdict still match, the scenario they name does not.
    let mut forged = package.clone();
    let commit = forged
        .minimized
        .aged
        .events
        .iter()
        .filter(|event| matches!(event.payload, eval_core::Payload::Commit { .. }))
        .map(|event| Element::Event {
            history: History::Aged,
            id: event.id.clone(),
        })
        .find(|element| {
            let mut deleted = forged.shrink.deleted.clone();
            deleted.insert(element.clone());
            forged.shrink.candidates.iter().any(|record| {
                record.deleted == deleted
                    && matches!(
                        record.verdict,
                        CandidateVerdict::Slipped { .. } | CandidateVerdict::NotReproduced
                    )
            })
        })
        .expect("a counted commit");
    let mut deleted = forged.shrink.deleted.clone();
    deleted.insert(commit);
    for record in &mut forged.shrink.candidates {
        if record.deleted == deleted {
            record.scenario_digest = "00".repeat(32);
        }
    }
    assert_eq!(
        forged.count_triggered()["commit"],
        4,
        "a record under a foreign digest is not evidence for this deletion"
    );
    assert_eq!(
        forged.validate(),
        Err(WitnessError::RecipeMultiplicitiesDisagree)
    );
}

#[test]
fn a_parsed_package_holds_every_integer_to_the_canonical_range() {
    let (value, _) = package().serialize(&redactor(), ARTIFACT_BYTES).unwrap();
    let mut huge = value.clone();
    huge["shrink"]["max_replays"] = serde_json::json!(9_007_199_254_740_992u64);
    assert!(
        matches!(parse_witness(&huge), Err(WitnessError::Shape(_))),
        "a budget Bun cannot represent exactly is refused at the parser, as the serializer refuses it"
    );
}

#[test]
fn the_coverage_signature_names_only_registered_markers() {
    let mut package = package();
    package
        .original
        .coverage
        .insert("flt_shrink_marker_nobody_registered".to_string());
    assert_eq!(
        package.validate(),
        Err(WitnessError::UnregisteredMarker {
            name: "flt_shrink_marker_nobody_registered".to_string()
        }),
        "an unregistered marker is not a behaviour the run showed"
    );
    assert!(matches!(
        package.serialize(&redactor(), ARTIFACT_BYTES),
        Err(WitnessError::UnregisteredMarker { .. })
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
    assert_eq!(
        witness.serialize(&redactor(), ARTIFACT_BYTES).err(),
        Some(WitnessError::LiveRelabelledReplayable)
    );
    witness.replayable = false;
    witness.validate().unwrap();
}

#[test]
fn the_serializer_requires_the_verbatim_claim_boundary_and_rejects_forbidden_claims() {
    let mut witness = package();
    witness.claim_boundary.exclusions.pop();
    assert_eq!(
        witness.serialize(&redactor(), ARTIFACT_BYTES).err(),
        Some(WitnessError::ClaimBoundaryMismatch)
    );

    let mut claims = package();
    claims.original.predicate.profile_digest = "proves live-model quality".to_string();
    claims.shrink.predicate.profile_digest = claims.original.predicate.profile_digest.clone();
    assert_eq!(
        claims.serialize(&redactor(), ARTIFACT_BYTES).err(),
        Some(WitnessError::ForbiddenClaim {
            path: "/original/predicate/profile_digest".to_string(),
            phrase: "live-model quality".to_string(),
        })
    );
    let mut cased = package();
    cased
        .original
        .coverage
        .insert("Scheduler-Order Independence shown".to_string());
    let refused = cased.serialize(&redactor(), ARTIFACT_BYTES).err().unwrap();
    assert_eq!(
        refused,
        WitnessError::ForbiddenClaim {
            path: "/original/coverage[0]".to_string(),
            phrase: "scheduler-order independence".to_string(),
        },
        "case does not hide a claim and an array leaf names its index"
    );
}

#[test]
fn residue_drift_refuses_and_limits_apply_before_publication() {
    let package = package();
    let mut drifted = package.residue.clone();
    let moved = drifted.pop_first().unwrap();
    residue_drift(&package.residue, &package.residue).unwrap();
    assert_eq!(
        residue_drift(&package.residue, &drifted),
        Err(WitnessError::ResidueDrift {
            missing: BTreeSet::from([moved]),
            unexpected: BTreeSet::new(),
        })
    );
    assert!(matches!(
        package.serialize(&redactor(), 16),
        Err(WitnessError::TooLarge { bound: 16, .. })
    ));
    let mut leaking = package.clone();
    leaking.original.predicate.profile_digest =
        "Authorization: Bearer sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcd".to_string();
    leaking.shrink.predicate.profile_digest = leaking.original.predicate.profile_digest.clone();
    assert_eq!(
        leaking.serialize(&redactor(), ARTIFACT_BYTES).err(),
        Some(WitnessError::RedactionRefused(
            RedactionErrorKind::SecretDetected
        ))
    );
}
