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

use daemon::search_catchup::{Blocked, EpisodeEnd, EpisodeEvent, EpisodeReport};
use eval_core::{
    Approval, Coverage, Cut, CutOutcome, EffectLedger, EffectOutcome, EffectState, ExecutionMode,
    Expected, FaultReport, MARKERS, ProfileError, Scale, SearchEpisodeFault, parse_fault_report,
    parse_manifest,
};
use fault::{
    Config, MANIFEST_FILE, REPORT_FILE, Run, RunError, Witness, check_expectations,
    declare_lost_reply_episode, lost_reply_episode, read_back, receipt_lost_reply_episode,
};

const MESSAGES: u32 = 40;
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
    let run = fault::run(&config(out.clone(), budget().unwrap_or(600_000))).unwrap();
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
        report.coverage.declared.len() >= 9,
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
    for kind in ["search_episode", "external_lock_holder"] {
        assert!(actions.contains(kind), "{actions:?} lacks {kind}");
    }
    for episode in &report.episodes {
        assert_eq!(episode.heal, episode.action.heal());
        assert!(!episode.layer_contract.is_empty());
        assert!(episode.kill.is_none(), "no kill episode in this campaign");
    }
    assert!(report.safety_checks_while_armed >= report.episodes.len() as u64);
    assert!(report.liveness.is_none(), "liveness is a separate mode");

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
}

fn a_lost_reply_stays_unknown_until_readback_at_after_recovery_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    let effects = &run.report.effects.effects;
    let lost: Vec<_> = effects.iter().filter(|(_, e)| e.reply_lost).collect();
    assert_eq!(lost.len(), 2, "{effects:?}");
    for (identity, effect) in &lost {
        assert!(
            effect.read_back,
            "{identity} was read back at AfterRecovery"
        );
        assert_ne!(effect.outcome, EffectOutcome::Unknown, "{identity}");
        assert!(effect.acknowledged <= effect.observed && effect.observed <= effect.attempted);
        assert_eq!(effect.attempted, 1, "{identity}");
        assert_eq!(
            effect.expected,
            Expected::Exactly {
                state: eval_core::EffectState::Applied
            },
            "{identity}: the production reconciliation committed and the durable checkpoint says so"
        );
    }
    assert!(lost.iter().any(|(id, _)| id.starts_with("search_commit:")));
    assert!(lost.iter().any(|(id, _)| id.starts_with("search_ack:")));
    assert!(run.report.effects.unknown().is_empty());
}

fn an_external_lock_holder_blocks_then_releases_scenario(campaign: &Campaign) {
    let run = &campaign.run;
    assert_eq!(run.report.coverage.receipted["lock_blocked"], 1);
    assert_eq!(run.report.coverage.receipted["lock_released"], 1);
}

const SCENARIOS: [fn(&Campaign); 3] = [
    the_fault_campaign_receipts_every_declared_cut_scenario,
    a_lost_reply_stays_unknown_until_readback_at_after_recovery_scenario,
    an_external_lock_holder_blocks_then_releases_scenario,
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
fn an_external_lock_holder_blocks_then_releases() {
    budget_or_panic();
    an_external_lock_holder_blocks_then_releases_scenario(&campaign(&mut Coverage::default()));
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
    let pending: BTreeSet<&str> = [
        "flt_kill_barrier_read_before_kill",
        "flt_liveness_bounds_met_with_faults_armed",
        "sls_embedding_publication_held_then_released",
        "flt_r11_recorded_as_expected_refusal",
        "flt_r24_recorded_as_expected_refusal",
        "flt_corruption_detected_at_quiescence",
        "flt_artifact_fault_named_errno",
    ]
    .into_iter()
    .collect();
    let missing: BTreeSet<&str> = owned.difference(&fired).copied().collect();
    assert_eq!(
        missing, pending,
        "every marker this suite owns fires here except those whose scenarios are not in this shell yet"
    );
}

#[test]
fn an_unapproved_profile_refuses_before_any_store_opens() {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let mut config = config(out.clone(), 600_000);
    config.approval = None;
    let error = fault::run(&config).err().unwrap();
    assert!(
        matches!(error, RunError::Profile(ProfileError::NotApproved { .. })),
        "{error}"
    );
    assert!(!out.exists());
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
}

fn reached(through: i64) -> EpisodeReport {
    EpisodeReport {
        target: through,
        acknowledged_through: through,
        batches_applied: 1,
        commits_consumed: 1,
        end: EpisodeEnd::ReachedTarget,
    }
}

fn window(through: i64) -> [EpisodeEvent; 4] {
    [
        EpisodeEvent::LocalStaged { through },
        EpisodeEvent::LocalReleased { through },
        EpisodeEvent::AcknowledgementRequested { through },
        EpisodeEvent::Acknowledged { through },
    ]
}

const REPLY_LOSS: [(SearchEpisodeFault, &str); 2] = [
    (SearchEpisodeFault::LoseLocalCommitReply, "search_commit"),
    (SearchEpisodeFault::LoseAcknowledgementReply, "search_ack"),
];

#[test]
fn a_lost_reply_episode_that_does_not_reach_its_target_is_refused() {
    for (fault, _) in REPLY_LOSS {
        let mut witness = Witness::new();
        declare_lost_reply_episode(&mut witness, "lost", 7, fault);
        let report = EpisodeReport {
            end: EpisodeEnd::Blocked(Blocked::LocalCommitUnresolved),
            acknowledged_through: 4,
            ..reached(9)
        };
        let error = receipt_lost_reply_episode(&mut witness, "lost", fault, &report, &window(9))
            .unwrap_err();
        assert!(matches!(error, RunError::Unexpected { .. }), "{error}");
        assert!(!witness.cuts.receipted.contains_key("lost"), "{fault:?}");
        assert!(witness.effects.effects.is_empty(), "{fault:?}");
    }
}

#[test]
fn every_window_of_a_lost_reply_episode_loses_its_reply() {
    for (fault, kind) in REPLY_LOSS {
        let mut witness = Witness::new();
        declare_lost_reply_episode(&mut witness, "lost", 7, fault);
        let events: Vec<EpisodeEvent> = window(5).into_iter().chain(window(9)).collect();
        receipt_lost_reply_episode(&mut witness, "lost", fault, &reached(9), &events).unwrap();
        assert_eq!(
            witness.effects.unknown(),
            [format!("{kind}:5"), format!("{kind}:9")]
                .into_iter()
                .collect(),
            "{fault:?}"
        );
        for effect in witness.effects.effects.values() {
            assert!(effect.reply_lost && effect.attempted == 1, "{effect:?}");
        }
    }
}

#[test]
fn a_lost_reply_without_a_matching_fixed_expectation_refuses_the_run() {
    let identity = "search_commit:9";
    let mut effects = EffectLedger::default();
    effects.attempt(identity);
    effects.lose_reply(identity).unwrap();
    let fixed = BTreeMap::from([(identity.to_string(), EffectState::Applied)]);
    assert!(
        check_expectations(&fixed, &effects).is_err(),
        "not read back"
    );
    effects
        .read_back(identity, EffectState::NotApplied)
        .unwrap();
    assert!(
        check_expectations(&BTreeMap::new(), &effects).is_err(),
        "a lost reply the campaign fixed no expectation for"
    );
    assert!(
        check_expectations(&fixed, &effects).is_err(),
        "read back not applied"
    );
    effects.read_back(identity, EffectState::Applied).unwrap();
    check_expectations(&fixed, &effects).unwrap();
    let stray = BTreeMap::from([("search_ack:9".to_string(), EffectState::Applied)]);
    assert!(
        check_expectations(&stray, &effects).is_err(),
        "an expectation for an effect the campaign never lost"
    );
}

#[test]
fn a_read_back_after_later_catch_up_is_refused_as_masked() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = aging::Stores::open(root.path(), plan.rendering.clone());
    aging::live(&mut stores, &plan.steps[..3]);
    let mut witness = Witness::new();
    stores.apply(&plan.steps[3]);
    lost_reply_episode(
        &mut stores,
        &mut witness,
        "lost",
        3,
        plan.steps[3].now_ms,
        SearchEpisodeFault::LoseLocalCommitReply,
    )
    .unwrap();
    stores.apply(&plan.steps[4]);
    stores.drain(plan.steps[4].now_ms);
    let closed = stores.close();
    let error = read_back(closed.root(), &mut witness).unwrap_err();
    assert!(matches!(error, RunError::ReadBackMasked { .. }), "{error}");
    assert_eq!(
        witness.effects.unknown().len(),
        1,
        "a masked read-back resolves nothing"
    );
}
