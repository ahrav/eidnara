#![cfg(all(unix, feature = "test-support"))]

//! The shrink shell: fresh-process replays at the pinned cut, the
//! replay-effect ledger under child death and timeout, and the published
//! witness package.

mod support;

#[path = "../examples/eval_runner/aging.rs"]
#[allow(dead_code)]
mod aging;
#[path = "../examples/eval_runner/campaign.rs"]
#[allow(dead_code)]
mod campaign;
#[path = "../examples/eval_runner/fault.rs"]
#[allow(dead_code)]
mod fault;
#[path = "../examples/eval_runner/shrink.rs"]
#[allow(dead_code)]
mod shrink;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use context_core::canonical_json::protocol_digest;
use eval_core::{
    Approval, CandidateVerdict, Cut, CutOutcome, Element, EventId, FailureClass, History,
    Minimality, NotEstablishedReason, Oracle, OracleRefused, ProfileError, ReplayOutcome, Scale,
    Scenario, ShrinkRefused, Transformation, UnknownReason, WITNESS_DIGEST_PROTOCOL, WitnessClass,
    WitnessError, parse_manifest, parse_witness, residue_drift,
};
use serde_json::{Value, json};
use shrink::{BARRIER, ChildArgs, Config, MANIFEST_FILE, Replayed, RunError, WITNESS_FILE};

const COMMITS: u32 = 8;
const STUBBORN: &str = "repository:repository-0:2";
const SUITE: &str = "crates/daemon/tests/eval_shrink.rs::";

fn reexec(entrypoint: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        entrypoint,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    command
}

/// The replays re-execute this test binary at the child entrypoint.
fn spawn_child(_: &ChildArgs) -> Command {
    reexec("shrink_child_entrypoint_reexecuted_by_the_parent")
}

#[test]
#[ignore = "re-executed by the replays with their environment set"]
fn shrink_child_entrypoint_reexecuted_by_the_parent() {
    let Some(args) = ChildArgs::from_env() else {
        return;
    };
    shrink::child_main(&args);
}

fn death_log() -> PathBuf {
    std::env::temp_dir().join(format!("eidnara-shrink-deaths-{}", std::process::id()))
}

/// A candidate without the stubborn commit gets a child that records its
/// death and exits before any barrier.
fn spawn_dying_without_stubborn(args: &ChildArgs) -> Command {
    if has_stubborn(args) {
        return spawn_child(args);
    }
    let mut command = reexec("shrink_child_dies_before_its_barrier");
    command.env("EIDNARA_EVAL_SHRINK_DEATH_LOG", death_log());
    command
}

#[test]
#[ignore = "re-executed by the death test"]
fn shrink_child_dies_before_its_barrier() {
    if let Ok(log) = std::env::var("EIDNARA_EVAL_SHRINK_DEATH_LOG") {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(log)
            .unwrap();
        std::io::Write::write_all(&mut file, b"died\n").unwrap();
    }
}

/// A candidate without the stubborn commit gets a child that never answers.
fn spawn_sleeping_without_stubborn(args: &ChildArgs) -> Command {
    if has_stubborn(args) {
        return spawn_child(args);
    }
    reexec("shrink_child_sleeps_past_the_timeout")
}

/// The digest of the scenario the parent sent: the key it reads answers under.
fn scenario_digest(args: &ChildArgs) -> String {
    let scenario: Scenario =
        serde_json::from_slice(&std::fs::read(&args.scenario).unwrap()).unwrap();
    scenario.digest()
}

fn has_stubborn(args: &ChildArgs) -> bool {
    let scenario: Scenario =
        serde_json::from_slice(&std::fs::read(&args.scenario).unwrap()).unwrap();
    scenario
        .aged
        .events
        .iter()
        .any(|event| event.id.0 == STUBBORN)
}

#[test]
#[ignore = "re-executed by the timeout test"]
fn shrink_child_sleeps_past_the_timeout() {
    if std::env::var(shrink::CHILD_ARGS).is_ok() {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// Every child reports one residue entry fewer than this build declares.
fn spawn_drifting(_: &ChildArgs) -> Command {
    reexec("shrink_child_reports_a_drifted_residue")
}

#[test]
#[ignore = "re-executed by the drift test"]
fn shrink_child_reports_a_drifted_residue() {
    let Some(args) = ChildArgs::from_env() else {
        return;
    };
    let mut residue = shrink::residue();
    residue.pop_first();
    let scenario_digest = scenario_digest(&args);
    let outcome = ReplayOutcome::Passed;
    let replayed = Replayed {
        trace_digest: shrink::trace_digest(&scenario_digest, &outcome, 0),
        scenario_digest,
        outcome,
        residue,
    };
    println!("{BARRIER} {}", serde_json::to_string(&replayed).unwrap());
}

/// Every child answers honestly but under a trace digest of its own choosing.
fn spawn_forged_trace(_: &ChildArgs) -> Command {
    reexec("shrink_child_reports_a_forged_trace")
}

#[test]
#[ignore = "re-executed by the forged-trace test"]
fn shrink_child_reports_a_forged_trace() {
    let Some(args) = ChildArgs::from_env() else {
        return;
    };
    let mut replayed = shrink::replayed(&args);
    replayed.trace_digest = "ab".repeat(32);
    println!("{BARRIER} {}", serde_json::to_string(&replayed).unwrap());
}

/// Every child answers `Failed` under a predicate pinned at another cut.
fn spawn_foreign_predicate(_: &ChildArgs) -> Command {
    reexec("shrink_child_reports_a_foreign_predicate")
}

/// The original is answered honestly; every candidate answers `Failed` under
/// a predicate pinned at another cut.
fn spawn_foreign_predicate_for_candidates(args: &ChildArgs) -> Command {
    if scenario_digest(args) == shrink::scenario(COMMITS).0.digest() {
        return spawn_child(args);
    }
    reexec("shrink_child_reports_a_foreign_predicate")
}

#[test]
#[ignore = "re-executed by the foreign-predicate test"]
fn shrink_child_reports_a_foreign_predicate() {
    let Some(args) = ChildArgs::from_env() else {
        return;
    };
    let scenario_digest = scenario_digest(&args);
    let outcome = ReplayOutcome::Failed {
        predicate: eval_core::FailurePredicate {
            oracle: args.oracle,
            checkpoint: Cut::EndOfRun,
            profile_digest: args.profile_digest,
            witness_class: WitnessClass::Failure {
                task: "first-commit".to_string(),
                class: FailureClass::Interference,
            },
        },
    };
    let replayed = Replayed {
        trace_digest: shrink::trace_digest(&scenario_digest, &outcome, 0),
        scenario_digest,
        outcome,
        residue: shrink::residue(),
    };
    println!("{BARRIER} {}", serde_json::to_string(&replayed).unwrap());
}

/// Every child evaluates the original scenario, whatever candidate it was
/// sent: a structurally valid answer for the wrong scenario.
fn spawn_answering_for_the_original(_: &ChildArgs) -> Command {
    reexec("shrink_child_answers_about_the_original_scenario")
}

#[test]
#[ignore = "re-executed by the misdirected-child test"]
fn shrink_child_answers_about_the_original_scenario() {
    let Some(args) = ChildArgs::from_env() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&shrink::scenario(COMMITS).0).unwrap(),
    )
    .unwrap();
    shrink::child_main(&ChildArgs {
        scenario: path,
        ..args
    });
}

fn approval() -> Approval {
    Approval {
        approved_by: "maintainer".to_string(),
        approved_at_run_id: "ab".repeat(32),
    }
}

fn config(publish: PathBuf) -> Config {
    Config {
        scale: Scale::S0,
        commits: COMMITS,
        elapsed_bound_ms: 600_000,
        approval: Some(approval()),
        publish,
        oracle: Oracle::RequiredCommits {
            failing_at: 3,
            slipping_at: 6,
        },
        replay_timeout: Duration::from_secs(120),
    }
}

fn commits(scenario: &Scenario) -> usize {
    scenario
        .aged
        .events
        .iter()
        .filter(|e| matches!(e.payload, eval_core::Payload::Commit { .. }))
        .count()
}

/// Runs the child entrypoint in a fresh process over `scenario` and parses
/// its barrier line.
fn replay_in_fresh_process(scenario: &Scenario, args: &ChildArgs) -> Replayed {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scenario.json");
    std::fs::write(&path, serde_json::to_vec(scenario).unwrap()).unwrap();
    let args = ChildArgs {
        scenario: path,
        ..args.clone()
    };
    let mut command = spawn_child(&args);
    args.env(&mut command);
    let mut child = command.stdout(Stdio::piped()).spawn().unwrap();
    let line = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
        .find(|line| line.contains(BARRIER))
        .expect("the child printed its barrier");
    assert!(child.wait().unwrap().success());
    serde_json::from_str(line[line.find(BARRIER).unwrap() + BARRIER.len()..].trim()).unwrap()
}

#[test]
fn a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published() {
    let publish = tempfile::tempdir().unwrap();
    let config = config(publish.path().join("out"));
    let run = shrink::run(&config, spawn_child).unwrap();

    let expected = WitnessClass::Failure {
        task: "first-commit".to_string(),
        class: FailureClass::Interference,
    };
    let ReplayOutcome::Failed { predicate } = &run.original.outcome else {
        panic!("the original fails: {:?}", run.original);
    };
    assert_eq!(predicate.witness_class, expected);
    assert_eq!(predicate.checkpoint, Cut::AtQuiescence);
    assert_eq!(predicate.oracle, config.oracle);
    let witness = &run.witness;
    assert_eq!(witness.original.predicate, *predicate);
    assert_eq!(
        commits(&witness.minimized),
        6,
        "one fewer commit slips the class"
    );
    assert!(witness.minimized.episodes.is_empty());
    assert_eq!(
        witness.shrink.minimality,
        Minimality::OneMinimal {
            transformations: vec![
                Transformation::FaultEpisodeRemoval,
                Transformation::EventDeletion
            ]
        }
    );
    assert!(
        witness
            .shrink
            .candidates
            .iter()
            .any(|r| matches!(r.verdict, CandidateVerdict::Slipped { .. })),
        "a five-commit candidate was rejected as slipped"
    );
    assert_eq!(witness.shrink.unknown_candidates, 0);
    assert!(witness.replayable);

    // The minimized scenario reproduces the precise predicate in two more
    // fresh processes, and their semantic trace digests agree.
    let args = ChildArgs {
        scenario: PathBuf::new(),
        oracle: config.oracle.clone(),
        checkpoint: Cut::AtQuiescence,
        profile_digest: predicate.profile_digest.clone(),
    };
    let first = replay_in_fresh_process(&witness.minimized, &args);
    let second = replay_in_fresh_process(&witness.minimized, &args);
    assert_eq!(
        first.outcome,
        ReplayOutcome::Failed {
            predicate: predicate.clone()
        }
    );
    assert_eq!(
        first, second,
        "two fresh processes agree on outcome and trace digest"
    );
    assert_ne!(
        first.trace_digest, witness.original.trace_digest,
        "a different scenario, a different trace"
    );
    assert_eq!(
        replay_in_fresh_process(&shrink::scenario(COMMITS).0, &args).trace_digest,
        witness.original.trace_digest,
        "the original replays to its recorded trace digest"
    );
    residue_drift(&witness.residue, &first.residue).unwrap();

    // The compact recipe regenerates the minimized worlds.
    let recipe = witness
        .recipe
        .as_ref()
        .expect("six commits remain: a count triggers it");
    assert_eq!(
        recipe.multiplicities,
        BTreeMap::from([("commit".to_string(), 5)]),
        "five of the six remaining commits slip the class when deleted alone; the sixth is evidence"
    );

    // Published atomically, byte-identical to what parses back, digested in
    // the manifest.
    let published = std::fs::read(config.publish.join(WITNESS_FILE)).unwrap();
    assert_eq!(published, run.witness_bytes);
    let value: Value = serde_json::from_slice(&published).unwrap();
    assert_eq!(parse_witness(&value).unwrap(), *witness);
    let manifest = parse_manifest(
        &serde_json::from_slice(&std::fs::read(config.publish.join(MANIFEST_FILE)).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.witness_digest,
        protocol_digest(WITNESS_DIGEST_PROTOCOL, &value).unwrap()
    );
    assert_eq!(manifest.eval_run_id, witness.original.eval_run_id);
    assert_eq!(
        manifest.component_versions.task_corpus,
        format!("generated:{:#x}", shrink::SEED),
        "the corpus names the seed the run identity was built from"
    );
    assert_eq!(manifest.component_versions.execution_image, "fresh-process");
    assert_eq!(
        manifest.run_identity.config["tasks_per_world"],
        json!(witness.minimized.tasks.len()),
        "the pinned profile describes the workload the scenario carries"
    );
    assert_eq!(
        manifest.run_identity.config["name"],
        json!("s0-suite-c-shrink")
    );
    assert_eq!(
        manifest.residue,
        shrink::residue(),
        "the manifest declares the residue the replays and the witness declare"
    );
    assert_eq!(
        manifest.run_identity.scenario["replay_timeout"],
        json!({
            "secs": config.replay_timeout.as_secs(),
            "nanos": config.replay_timeout.subsec_nanos(),
        }),
        "the wait a child is allowed decides its verdict, so the identity pins it whole"
    );
    assert_eq!(
        manifest.cut_receipts,
        vec![eval_core::CutReceipt {
            cut: Cut::AtQuiescence,
            outcome: CutOutcome::Reached
        }]
    );
    let names: BTreeSet<String> = std::fs::read_dir(&config.publish)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([WITNESS_FILE.to_string(), MANIFEST_FILE.to_string()]),
        "no staged file remains"
    );
    let fired = run.coverage.fired();
    assert!(fired.contains("flt_shrink_fresh_process_reproduced"));
    assert!(fired.contains("flt_shrink_slipped_candidate_rejected"));
    assert!(
        !fired.contains("flt_shrink_unknown_effect_preserved"),
        "no replay answered unknown, so the marker did not fire"
    );
    assert!(
        witness
            .original
            .coverage
            .contains("flt_shrink_slipped_candidate_rejected")
    );

    // A second run into a directory that already holds a witness is refused
    // before anything is replayed.
    assert!(matches!(
        shrink::run(&config, spawn_child),
        Err(RunError::Publish { .. })
    ));
}

#[test]
fn a_child_that_dies_before_its_barrier_is_retried_then_unknown_and_kept() {
    let _ = std::fs::remove_file(death_log());
    let publish = tempfile::tempdir().unwrap();
    let config = config(publish.path().join("out"));
    let run = shrink::run(&config, spawn_dying_without_stubborn).unwrap();
    let witness = &run.witness;
    let stubborn = Element::Event {
        history: History::Aged,
        id: EventId(STUBBORN.to_string()),
    };
    assert!(
        witness.minimized.elements().contains(&stubborn),
        "the element whose deletion never answered stays"
    );
    assert!(witness.shrink.unknown_candidates > 0);
    let unknown: Vec<_> = witness
        .shrink
        .candidates
        .iter()
        .filter(|r| r.deleted.contains(&stubborn))
        .collect();
    assert!(!unknown.is_empty());
    for record in &unknown {
        assert!(
            matches!(
                record.verdict,
                CandidateVerdict::Unknown {
                    reason: UnknownReason::ChildExitedBeforeBarrier
                } | CandidateVerdict::InvalidPair { .. }
            ),
            "never NotReproduced: {:?}",
            record.verdict
        );
    }
    assert!(matches!(
        witness.shrink.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { .. }
        }
    ));
    let distinct_unknown = witness
        .shrink
        .candidates
        .iter()
        .filter(|r| matches!(r.verdict, CandidateVerdict::Unknown { .. }))
        .map(|r| r.scenario_digest.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let deaths = std::fs::read_to_string(death_log())
        .unwrap()
        .lines()
        .count();
    assert_eq!(
        deaths,
        2 * distinct_unknown,
        "every dying candidate was retried once under its key"
    );
    assert!(
        run.coverage
            .fired()
            .contains("flt_shrink_unknown_effect_preserved")
    );
    assert!(
        run.coverage
            .fired()
            .contains("flt_shrink_slipped_candidate_rejected"),
        "candidates that answered still slipped"
    );
    run.coverage
        .complete(SUITE)
        .expect("this run fires every marker the suite owns");
    assert!(witness.recipe.is_none(), "no 1-minimality, no recipe");
    let _ = std::fs::remove_file(death_log());
}

#[test]
fn a_child_that_never_answers_is_cancelled_and_unknown() {
    let publish = tempfile::tempdir().unwrap();
    let mut config = config(publish.path().join("out"));
    config.replay_timeout = Duration::from_secs(2);
    let run = shrink::run(&config, spawn_sleeping_without_stubborn).unwrap();
    let cancelled = run
        .witness
        .shrink
        .candidates
        .iter()
        .filter(|r| {
            r.verdict
                == CandidateVerdict::Unknown {
                    reason: UnknownReason::Cancelled,
                }
        })
        .count();
    assert!(cancelled > 0, "a timed-out replay is a cancelled effect");
    let stubborn = Element::Event {
        history: History::Aged,
        id: EventId(STUBBORN.to_string()),
    };
    for record in &run.witness.shrink.candidates {
        if record.deleted.contains(&stubborn) {
            assert_ne!(
                record.verdict,
                CandidateVerdict::NotReproduced,
                "a cancelled effect never answered"
            );
        }
    }
    assert!(run.witness.minimized.elements().contains(&stubborn));
    assert!(matches!(
        run.witness.shrink.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { .. }
        }
    ));
}

#[test]
fn a_child_whose_residue_drifted_refuses_the_run() {
    let publish = tempfile::tempdir().unwrap();
    let config = config(publish.path().join("out"));
    let refused = shrink::run(&config, spawn_drifting).err().unwrap();
    match refused {
        RunError::Witness(WitnessError::ResidueDrift {
            missing,
            unexpected,
        }) => {
            assert_eq!(missing.len(), 1);
            assert!(unexpected.is_empty());
        }
        other => panic!("expected residue drift, got {other:?}"),
    }
    assert!(!config.publish.join(WITNESS_FILE).exists());
}

#[test]
fn a_child_predicate_pinned_elsewhere_is_refused_and_nothing_is_published() {
    let publish = tempfile::tempdir().unwrap();
    let config = config(publish.path().join("out"));
    let refused = shrink::run(&config, spawn_foreign_predicate).err().unwrap();
    assert!(
        matches!(refused, RunError::ForeignPredicate { .. }),
        "the child's cut is not the pinned one: {refused:?}"
    );
    assert!(!config.publish.join(WITNESS_FILE).exists());
}

#[test]
fn a_commit_count_the_scenario_cannot_carry_is_refused_before_anything_runs() {
    let publish = tempfile::tempdir().unwrap();
    for commits in [0, 1, 78] {
        let mut config = config(publish.path().join(format!("commits-{commits}")));
        config.commits = commits;
        let refused = shrink::run(&config, spawn_child).err().unwrap();
        assert!(
            matches!(refused, RunError::Commits(_)),
            "{commits} commits: {refused:?}"
        );
        assert!(
            !config.publish.exists(),
            "{commits} commits published a root"
        );
    }
}

#[test]
fn an_answer_for_another_scenario_is_unknown_and_shrinks_nothing() {
    let publish = tempfile::tempdir().unwrap();
    let config = config(publish.path().join("out"));
    let run = shrink::run(&config, spawn_answering_for_the_original).unwrap();
    let witness = &run.witness;
    assert!(
        witness.shrink.deleted.is_empty(),
        "an answer for the original is no evidence about a candidate: {:?}",
        witness.shrink.deleted
    );
    for record in &witness.shrink.candidates[1..] {
        assert!(
            matches!(
                record.verdict,
                CandidateVerdict::Unknown {
                    reason: UnknownReason::ReadBackFailed
                } | CandidateVerdict::InvalidPair { .. }
            ),
            "{:?}",
            record.verdict
        );
    }
    assert!(matches!(
        witness.shrink.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { .. }
        }
    ));
}

#[test]
fn a_candidate_answered_under_a_foreign_predicate_is_unknown_not_slipped() {
    let publish = tempfile::tempdir().unwrap();
    let config = config(publish.path().join("out"));
    let run = shrink::run(&config, spawn_foreign_predicate_for_candidates).unwrap();
    let witness = &run.witness;
    assert!(witness.shrink.deleted.is_empty());
    for record in &witness.shrink.candidates[1..] {
        assert!(
            matches!(
                record.verdict,
                CandidateVerdict::Unknown {
                    reason: UnknownReason::ReadBackFailed
                } | CandidateVerdict::InvalidPair { .. }
            ),
            "a foreign predicate is no rejection: {:?}",
            record.verdict
        );
    }
    assert!(matches!(
        witness.shrink.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { .. }
        }
    ));
}

#[test]
fn a_trace_digest_that_is_not_the_observation_s_is_no_answer() {
    let publish = tempfile::tempdir().unwrap();
    let config = config(publish.path().join("out"));
    let refused = shrink::run(&config, spawn_forged_trace).err().unwrap();
    assert!(
        matches!(
            refused,
            RunError::NoFailure {
                outcome: ReplayOutcome::Unknown {
                    reason: UnknownReason::ReadBackFailed
                }
            }
        ),
        "{refused:?}"
    );
    assert!(!config.publish.join(WITNESS_FILE).exists());
}

#[test]
fn an_inverted_oracle_is_refused_before_the_original_is_replayed() {
    let publish = tempfile::tempdir().unwrap();
    let mut config = config(publish.path().join("out"));
    config.oracle = Oracle::RequiredCommits {
        failing_at: 1_000,
        slipping_at: 3,
    };
    let refused = shrink::run(&config, spawn_child).err().unwrap();
    assert!(
        matches!(
            refused,
            RunError::Shrink(ShrinkRefused::InvalidOracle(
                OracleRefused::InvertedThresholds { .. }
            ))
        ),
        "{refused:?}"
    );
    assert!(!config.publish.exists(), "refused before the publish root");
}

#[test]
fn an_original_that_does_not_fail_or_an_unapproved_profile_is_refused() {
    let publish = tempfile::tempdir().unwrap();
    let mut config = config(publish.path().join("passing"));
    config.oracle = Oracle::RequiredCommits {
        failing_at: 1_000,
        slipping_at: 2_000,
    };
    assert!(matches!(
        shrink::run(&config, spawn_child),
        Err(RunError::NoFailure {
            outcome: ReplayOutcome::Passed
        })
    ));
    assert!(!config.publish.join(WITNESS_FILE).exists());

    let mut unapproved = self::config(publish.path().join("unapproved"));
    unapproved.approval = None;
    assert!(matches!(
        shrink::run(&unapproved, spawn_child),
        Err(RunError::Profile(ProfileError::NotApproved { .. }))
    ));
    assert!(!unapproved.publish.exists());
}

#[test]
fn the_shrink_flags_are_parsed_and_the_child_needs_its_environment() {
    let run_id = "ab".repeat(32);
    let flags = |commits: &str| {
        [
            "--scale",
            "s0",
            "--commits",
            commits,
            "--elapsed-bound-ms",
            "1000",
            "--approved-by",
            "m",
            "--approval-run-id",
            &run_id,
            "--publish",
            "/tmp/x",
        ]
        .map(String::from)
    };
    let config = shrink::config_from_args(flags("8")).unwrap();
    assert_eq!(config.commits, 8);
    assert_eq!(config.replay_timeout, Duration::from_secs(120));
    assert!(shrink::config_from_args(["--scale".to_string(), "s0".to_string()]).is_err());
    assert!(
        shrink::config_from_args(flags("1")).is_err(),
        "a rename needs two commits"
    );
    let largest = shrink::config_from_args(flags("77")).unwrap();
    shrink::scenario(largest.commits);
    let refused = shrink::config_from_args(flags("78"))
        .expect_err("78 commits exceed the aged world's 128-event bound");
    assert!(refused.starts_with("--commits:"), "{refused}");
    assert!(ChildArgs::from_env().is_none());
}
