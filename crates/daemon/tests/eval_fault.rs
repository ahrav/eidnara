#![cfg(all(unix, feature = "test-support"))]

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

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eval_core::{
    Approval, Coverage, Cut, CutOutcome, EffectOutcome, ExecutionMode, Expected, ExpectedRefusal,
    FaultAction, FaultReport, Heal, MARKERS, ProfileError, Scale, StoreFamily, parse_fault_report,
    parse_manifest,
};
use fault::{Config, MANIFEST_FILE, REPORT_FILE, Run, RunError};

const MESSAGES: u32 = 40;

/// The kill episodes re-execute this test binary at the child entrypoint.
fn spawn_child(args: &fault::ChildArgs) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "fault_child_entrypoint_reexecuted_by_the_parent",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    args.env(&mut command);
    command
}

/// The child owns its parked episode; an ignored-test sweep without the
/// parent's environment returns at once.
#[test]
#[ignore = "re-executed by the kill episodes with their environment set"]
fn fault_child_entrypoint_reexecuted_by_the_parent() {
    let Some(args) = fault::ChildArgs::from_env() else {
        return;
    };
    fault::child_main(&args);
}
const SUITE: &str = "crates/daemon/tests/eval_fault.rs::";

fn budget() -> Option<u64> {
    let variable = Scale::S0.budget_env();
    std::env::var(variable).ok().map(|text| {
        text.parse::<u64>()
            .unwrap_or_else(|e| panic!("{variable}={text:?} is not a millisecond budget: {e}"))
    })
}

fn config(publish: PathBuf, elapsed_bound_ms: u64) -> Config {
    Config {
        scale: Scale::S0,
        messages: MESSAGES,
        elapsed_bound_ms,
        approval: Some(Approval {
            approved_by: "maintainer".to_string(),
            approved_at_run_id: "ab".repeat(32),
        }),
        publish,
    }
}

struct Campaign {
    run: Run,
    out: PathBuf,
    _publish: tempfile::TempDir,
}

fn campaign(coverage: &mut Coverage) -> Campaign {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let run = fault::run(
        &config(out.clone(), budget().unwrap_or(600_000)),
        spawn_child,
    )
    .unwrap();
    for marker in run.coverage.fired() {
        coverage.record(marker).unwrap();
    }
    Campaign {
        run,
        out,
        _publish: publish,
    }
}

fn budget_or_panic() -> u64 {
    budget().unwrap_or_else(|| {
        panic!(
            "{} is unset; this test runs only under an explicit budget",
            Scale::S0.budget_env()
        )
    })
}

fn the_fault_campaign_receipts_every_declared_cut_scenario(campaign: &Campaign) {
    let (run, out) = (&campaign.run, &campaign.out);
    let report = &run.report;
    report.validate(&run.bounds).unwrap();
    assert_eq!(
        report.coverage.declared.len(),
        report.coverage.receipted.len()
    );
    assert!(
        report.coverage.declared.len() >= 20,
        "{:?}",
        report.coverage.declared
    );
    for cut in &report.cuts {
        assert_eq!(cut.outcome, CutOutcome::Reached, "{cut:?}");
    }
    assert_eq!(
        report.cuts.iter().map(|c| c.cut).collect::<Vec<_>>(),
        vec![
            Cut::AtQuiescence,
            Cut::AfterFaultPhase,
            Cut::AfterRecovery,
            Cut::EndOfRun
        ]
    );
    let actions: BTreeSet<String> = report
        .episodes
        .iter()
        .map(|e| {
            serde_json::to_value(&e.action).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    for kind in [
        "search_episode",
        "external_lock_holder",
        "artifact_ingest",
        "artifact_deletion",
        "corrupt_quiescent_file",
        "embedding_publication",
        "held_publication",
        "process_kill",
    ] {
        assert!(actions.contains(kind), "{actions:?} lacks {kind}");
    }
    for episode in &report.episodes {
        assert_eq!(episode.heal, episode.action.heal());
        assert!(!episode.layer_contract.is_empty());
        assert_eq!(episode.kill.is_some(), episode.action.is_kill());
    }
    assert!(report.safety_checks_while_armed >= report.episodes.len() as u64);
    assert!(
        report.liveness.is_some(),
        "liveness is reported as its own mode"
    );

    let published = serde_json::from_slice(&std::fs::read(out.join(REPORT_FILE)).unwrap()).unwrap();
    assert_eq!(
        parse_fault_report(&published, &run.bounds).unwrap(),
        *report
    );
    let manifest = parse_manifest(
        &serde_json::from_slice(&std::fs::read(out.join(MANIFEST_FILE)).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.execution_mode, ExecutionMode::Generate);
    assert_eq!(
        manifest.result_digest,
        FaultReport::result_digest(&published).unwrap()
    );
    assert_eq!(manifest.cut_receipts, report.cuts);
    assert_eq!(manifest.witness_digest, fault::witness_digest(report));
    assert!(!report.barriers.is_empty());
    let mut respawned = report.clone();
    for barrier in &mut respawned.barriers {
        barrier.pid = barrier.pid.wrapping_add(1);
    }
    assert_eq!(
        fault::witness_digest(&respawned),
        manifest.witness_digest,
        "the pid the OS gave a kill child is not witness evidence"
    );
}

fn a_lost_reply_stays_unknown_until_readback_at_after_recovery_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    let effects = &run.report.effects.effects;
    let lost: Vec<_> = effects.iter().filter(|(_, e)| e.reply_lost).collect();
    assert_eq!(lost.len(), 7, "{effects:?}");
    for (identity, effect) in &lost {
        assert!(
            effect.read_back,
            "{identity} was read back at AfterRecovery"
        );
        assert_ne!(effect.outcome, EffectOutcome::Unknown, "{identity}");
        assert!(effect.acknowledged <= effect.observed && effect.observed <= effect.attempted);
        assert_eq!(effect.attempted, 1, "{identity}");
    }
    let search: Vec<EffectOutcome> = lost
        .iter()
        .filter(|(id, _)| id.starts_with("search_commit:") || id.starts_with("search_ack:"))
        .map(|(_, e)| e.outcome)
        .collect();
    assert_eq!(search.len(), 5, "two lost replies and three killed effects");
    assert_eq!(
        search
            .iter()
            .filter(|o| **o == EffectOutcome::Applied)
            .count(),
        3,
        "the lost replies and the acknowledgement kill's local commit committed: {search:?}"
    );
    assert_eq!(
        search
            .iter()
            .filter(|o| **o == EffectOutcome::NotApplied)
            .count(),
        2,
        "the killed steps never committed their effect: {search:?}"
    );
    let embeddings: Vec<_> = lost
        .iter()
        .filter(|(id, _)| id.starts_with("embedding:"))
        .collect();
    assert_eq!(embeddings.len(), 2);
    let mut outcomes: Vec<_> = embeddings.iter().map(|(_, e)| e.outcome).collect();
    outcomes.sort_by_key(|o| format!("{o:?}"));
    assert_eq!(
        outcomes,
        vec![EffectOutcome::Applied, EffectOutcome::NotApplied],
        "a committed-then-lost reply reads back applied; a rolled-back one reads back not applied"
    );
    assert!(run.report.effects.unknown().is_empty());
}

fn deletion_bearing_catch_up_is_an_expected_refusal_and_a_permanent_stall_scenario(
    campaign: &Campaign,
) {
    let run = &campaign.run;
    let r11 = run
        .report
        .expected_refusals
        .iter()
        .find(|r| r.refusal == ExpectedRefusal::R11DeletionBearingCatchUp)
        .unwrap();
    assert!(
        r11.production_error.starts_with("DeletionUnpropagated"),
        "{r11:?}"
    );
    let episode = run
        .report
        .episodes
        .iter()
        .find(|e| e.id == r11.episode)
        .unwrap();
    assert_eq!(episode.scope.store, StoreFamily::SearchProjection);
    assert!(episode.layer_contract.contains("rebuilding"));
}

fn receipt_quota_exhaustion_is_an_expected_refusal_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    let r24 = run
        .report
        .expected_refusals
        .iter()
        .find(|r| r.refusal == ExpectedRefusal::R24ReceiptQuotaExhausted)
        .unwrap();
    assert_eq!(
        r24.production_error,
        "MemoryReviewerJobRefusal::MetadataQuota"
    );
    let episode = run
        .report
        .episodes
        .iter()
        .find(|e| e.id == r24.episode)
        .unwrap();
    assert_eq!(episode.scope.store, StoreFamily::Memory);
    assert_eq!(episode.heal, Heal::Released);
}

fn a_corrupted_quiescent_file_is_detected_before_any_store_opens_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    let episode = run
        .report
        .episodes
        .iter()
        .find(|e| e.action == FaultAction::CorruptQuiescentFile)
        .unwrap();
    assert_eq!(episode.heal, Heal::Reopen);
    assert_eq!(run.report.coverage.receipted["integrity_refused"], 1);
}

fn an_external_lock_holder_blocks_then_releases_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    assert_eq!(run.report.coverage.receipted["lock_blocked"], 1);
    assert_eq!(run.report.coverage.receipted["lock_released"], 1);
}

fn artifact_faults_fail_with_their_named_errno_and_heal_by_reopen_or_consumption_scenario(
    campaign: &Campaign,
) {
    let run = &campaign.run;
    let artifact: Vec<_> = run
        .report
        .episodes
        .iter()
        .filter(|e| {
            matches!(
                e.action,
                FaultAction::ArtifactIngest { .. } | FaultAction::ArtifactDeletion { .. }
            )
        })
        .collect();
    assert_eq!(artifact.len(), 7, "{artifact:?}");
    assert_eq!(run.report.coverage.receipted["artifact_fault_named"], 6);
    let heals: Vec<Heal> = artifact.iter().map(|e| e.heal).collect();
    assert_eq!(
        heals.iter().filter(|h| **h == Heal::Reopen).count(),
        5,
        "every EIO latches CAS ingestion closed until reopen: {heals:?}"
    );
    assert_eq!(
        heals.iter().filter(|h| **h == Heal::Consumed).count(),
        2,
        "ENOSPC and the healed deletion are consumed: {heals:?}"
    );
}

fn a_test_binary_child_killed_at_a_named_cut_recovers_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    let kills: Vec<_> = run
        .report
        .episodes
        .iter()
        .filter(|e| e.action.is_kill())
        .collect();
    assert_eq!(kills.len(), 2);
    for episode in &kills {
        let label = episode.kill.as_ref().unwrap();
        assert_eq!(label.crash_model, eval_core::APPLICATION_CRASH);
        assert!(label.page_cache_intact);
        assert_eq!(label.killed_process, eval_core::TEST_BINARY_CHILD);
        assert_eq!(episode.heal, Heal::Reopen);
        let barrier = run
            .report
            .barriers
            .iter()
            .find(|b| b.episode == episode.id)
            .unwrap();
        let FaultAction::ProcessKill { cut } = &episode.action else {
            unreachable!()
        };
        assert_eq!(&barrier.cut, cut);
        assert!(barrier.line.ends_with(cut), "{barrier:?}");
        assert_eq!(barrier.signal, 9, "SIGKILL, not an exit status");
        assert!(barrier.pid > 0);
        let suffix = format!("@{}", episode.id);
        let effects: BTreeMap<&str, EffectOutcome> = run
            .report
            .effects
            .effects
            .iter()
            .filter(|(identity, _)| identity.ends_with(&suffix))
            .map(|(identity, effect)| (identity.split_once(':').unwrap().0, effect.outcome))
            .collect();
        let expected: BTreeMap<&str, EffectOutcome> = match cut.as_str() {
            "local_staged" => [("search_commit", EffectOutcome::NotApplied)].into(),
            "acknowledgement_requested" => [
                ("search_commit", EffectOutcome::Applied),
                ("search_ack", EffectOutcome::NotApplied),
            ]
            .into(),
            other => panic!("no kill cut {other}"),
        };
        assert_eq!(
            effects, expected,
            "{}: the crashed files hold exactly what committed before the cut",
            episode.id
        );
    }
    assert!(
        run.report.coverage.receipted["local_staged"] >= 2,
        "the lost-reply episodes and the kill each stage a batch"
    );
    assert!(run.report.coverage.receipted["acknowledgement_requested"] >= 2);
}

fn a_held_publication_admits_once_and_publishes_on_release_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    let held = run
        .report
        .episodes
        .iter()
        .find(|e| e.action == FaultAction::HeldPublication)
        .unwrap();
    assert_eq!(held.heal, Heal::Released);
    assert_eq!(run.report.coverage.receipted["publication_held"], 1);
    assert_eq!(run.report.coverage.receipted["publication_released"], 1);
}

fn liveness_bounds_are_met_with_outside_core_faults_armed_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    let liveness = run.report.liveness.as_ref().unwrap();
    liveness.verdict(&run.bounds).unwrap();
    assert_eq!(
        liveness.outside_core.len(),
        1,
        "the memory-store lock holder is the permanent outside-core fault"
    );
    assert_eq!(liveness.armed_at_bound, liveness.outside_core);
    assert_eq!(liveness.core.lanes.len(), 3);
    assert!(
        !liveness
            .core
            .lanes
            .contains(&eval_core::Lane::ReviewerCoordinatorPasses),
        "the reviewer coordinator is outside this campaign's core"
    );
    for (lane, progress) in &liveness.lanes {
        assert_eq!(progress.bound, lane.bound(&run.bounds), "{lane:?}");
        assert_eq!(
            progress.steps, progress.bound,
            "{lane:?} was driven to its bound"
        );
        assert!(
            progress.met_at.is_some_and(|k| k <= progress.bound),
            "{lane:?}: {progress:?}"
        );
        assert!(progress.holds_at_bound, "{lane:?}");
        assert!(progress.stalled_at.is_none(), "{lane:?}: {progress:?}");
        assert!(progress.blocked.is_none(), "{lane:?}: {progress:?}");
        assert!(
            progress.fresh_commits >= 8,
            "{lane:?} was fed fresh kernel work inside its window: {progress:?}"
        );
    }
    assert_eq!(
        liveness.permanent_stalls.len(),
        1,
        "R11 is reported as the permanent stall it is"
    );
    assert_eq!(
        run.report.coverage.receipted.get("claims_materialized"),
        Some(&run.bounds.materialization_episodes),
        "every materialization step leaves exactly the latest fed decision's claims live"
    );
    assert!(run.report.safety_checks_while_armed > run.bounds.catch_up_episodes);
}

const SCENARIOS: [fn(&Campaign); 10] = [
    a_test_binary_child_killed_at_a_named_cut_recovers_scenario,
    a_held_publication_admits_once_and_publishes_on_release_scenario,
    liveness_bounds_are_met_with_outside_core_faults_armed_scenario,
    the_fault_campaign_receipts_every_declared_cut_scenario,
    a_lost_reply_stays_unknown_until_readback_at_after_recovery_scenario,
    deletion_bearing_catch_up_is_an_expected_refusal_and_a_permanent_stall_scenario,
    receipt_quota_exhaustion_is_an_expected_refusal_scenario,
    a_corrupted_quiescent_file_is_detected_before_any_store_opens_scenario,
    an_external_lock_holder_blocks_then_releases_scenario,
    artifact_faults_fail_with_their_named_errno_and_heal_by_reopen_or_consumption_scenario,
];

/// One campaign in the default shards, every scenario asserted over it.
#[test]
fn the_fault_campaign_receipts_every_declared_cut() {
    let campaign = campaign(&mut Coverage::default());
    for scenario in SCENARIOS {
        scenario(&campaign);
    }
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn a_lost_reply_stays_unknown_until_readback_at_after_recovery() {
    budget_or_panic();
    a_lost_reply_stays_unknown_until_readback_at_after_recovery_scenario(&campaign(
        &mut Coverage::default(),
    ));
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn deletion_bearing_catch_up_is_an_expected_refusal_and_a_permanent_stall() {
    budget_or_panic();
    deletion_bearing_catch_up_is_an_expected_refusal_and_a_permanent_stall_scenario(&campaign(
        &mut Coverage::default(),
    ));
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn receipt_quota_exhaustion_is_an_expected_refusal() {
    budget_or_panic();
    receipt_quota_exhaustion_is_an_expected_refusal_scenario(&campaign(&mut Coverage::default()));
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn a_corrupted_quiescent_file_is_detected_before_any_store_opens() {
    budget_or_panic();
    a_corrupted_quiescent_file_is_detected_before_any_store_opens_scenario(&campaign(
        &mut Coverage::default(),
    ));
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn an_external_lock_holder_blocks_then_releases() {
    budget_or_panic();
    an_external_lock_holder_blocks_then_releases_scenario(&campaign(&mut Coverage::default()));
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn artifact_faults_fail_with_their_named_errno_and_heal_by_reopen_or_consumption() {
    budget_or_panic();
    artifact_faults_fail_with_their_named_errno_and_heal_by_reopen_or_consumption_scenario(
        &campaign(&mut Coverage::default()),
    );
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn a_test_binary_child_killed_at_a_named_cut_recovers() {
    budget_or_panic();
    a_test_binary_child_killed_at_a_named_cut_recovers_scenario(
        &campaign(&mut Coverage::default()),
    );
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn a_held_publication_admits_once_and_publishes_on_release() {
    budget_or_panic();
    a_held_publication_admits_once_and_publishes_on_release_scenario(&campaign(
        &mut Coverage::default(),
    ));
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn liveness_bounds_are_met_with_outside_core_faults_armed() {
    budget_or_panic();
    liveness_bounds_are_met_with_outside_core_faults_armed_scenario(&campaign(
        &mut Coverage::default(),
    ));
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn every_fault_marker_fires_across_the_scenarios() {
    budget_or_panic();
    let mut coverage = Coverage::default();
    let campaign = campaign(&mut coverage);
    for scenario in SCENARIOS {
        scenario(&campaign);
    }
    let owned: BTreeSet<&str> = MARKERS
        .iter()
        .filter(|m| m.test.starts_with(SUITE))
        .map(|m| m.name)
        .collect();
    let fired: BTreeSet<&str> = coverage.fired().iter().copied().collect();
    let missing: BTreeSet<&str> = owned.difference(&fired).copied().collect();
    assert!(missing.is_empty(), "{missing:?}");
    coverage.complete(SUITE).unwrap();
}

#[test]
fn an_unapproved_profile_refuses_before_any_store_opens() {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let mut config = config(out.clone(), 600_000);
    config.approval = None;
    let error = fault::run(&config, spawn_child).err().unwrap();
    assert!(
        matches!(error, RunError::Profile(ProfileError::NotApproved { .. })),
        "{error}"
    );
    assert!(!out.exists());
}

#[test]
fn a_gated_lane_drops_while_its_inference_is_held() {
    use host_runtime::local_embeddings::EmbeddingEngine;

    let lane = fault::GatedLane::new();
    let engine = Arc::clone(&lane.engine);
    drop(lane.runtime.spawn_blocking(move || engine.embed(&["held"])));
    let started = Instant::now();
    while lane.engine.calls() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the inference never reached the gate"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let engine = Arc::clone(&lane.engine);
    let (dropped, done) = mpsc::channel();
    std::thread::spawn(move || {
        drop(lane);
        let _ = dropped.send(());
    });
    done.recv_timeout(Duration::from_secs(10))
        .expect("dropping the lane releases its held inference");
    assert_eq!(
        engine.completed(),
        1,
        "the held inference ran to completion"
    );
}

#[test]
fn fault_markers_each_name_a_scenario_here() {
    let mine: Vec<_> = MARKERS
        .iter()
        .filter(|m| m.test.starts_with(SUITE))
        .collect();
    let scenarios: BTreeSet<&str> = [
        "the_fault_campaign_receipts_every_declared_cut",
        "a_lost_reply_stays_unknown_until_readback_at_after_recovery",
        "deletion_bearing_catch_up_is_an_expected_refusal_and_a_permanent_stall",
        "receipt_quota_exhaustion_is_an_expected_refusal",
        "a_corrupted_quiescent_file_is_detected_before_any_store_opens",
        "an_external_lock_holder_blocks_then_releases",
        "artifact_faults_fail_with_their_named_errno_and_heal_by_reopen_or_consumption",
        "a_test_binary_child_killed_at_a_named_cut_recovers",
        "liveness_bounds_are_met_with_outside_core_faults_armed",
        "a_held_publication_admits_once_and_publishes_on_release",
    ]
    .into_iter()
    .collect();
    for marker in &mine {
        let scenario = marker.test.strip_prefix(SUITE).unwrap();
        assert!(
            scenarios.contains(scenario),
            "{} names no scenario here",
            marker.name
        );
    }
    assert_eq!(mine.len(), 10);
    let _ = &Expected::Exactly {
        state: eval_core::EffectState::Applied,
    };
}
