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

use std::collections::BTreeSet;
use std::path::PathBuf;

use eval_core::{
    Approval, Coverage, Cut, CutOutcome, EffectOutcome, ExecutionMode, Expected, ExpectedRefusal,
    FaultAction, FaultReport, Heal, MARKERS, ProfileError, Scale, StoreFamily, parse_fault_report,
    parse_manifest,
};
use fault::{Config, MANIFEST_FILE, REPORT_FILE, Run, RunError, Witness};
use memory_store::MemoryStore;
use memory_store::memory_reviewer_jobs::{
    MemoryReviewerJobError, MemoryReviewerJobOutcome, MemoryReviewerJobRefusal,
};
use support::embedding_fixtures::PROJECT;

const MESSAGES: u32 = 24;
const QUOTA_NOW_MS: i64 = 1_000;
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
        "expected_refusal",
    ] {
        assert!(actions.contains(kind), "{actions:?} lacks {kind}");
    }
    for episode in &report.episodes {
        assert_eq!(episode.heal, episode.action.heal());
        assert!(!episode.layer_contract.is_empty());
        assert!(episode.kill.is_none(), "no kill episode in this campaign");
    }
    let on_the_drive = report
        .episodes
        .iter()
        .filter(|e| e.scope.store != StoreFamily::Memory)
        .count();
    assert!(
        report.safety_checks_while_armed >= on_the_drive as u64,
        "every episode on the aging drive's stores is followed by a safety check"
    );
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
    assert_eq!(lost.len(), 4, "{effects:?}");
    for (identity, effect) in &lost {
        assert!(
            effect.read_back,
            "{identity} was read back at AfterRecovery"
        );
        assert_ne!(effect.outcome, EffectOutcome::Unknown, "{identity}");
        assert!(effect.acknowledged <= effect.observed && effect.observed <= effect.attempted);
        assert_eq!(effect.attempted, 1, "{identity}");
    }
    let commit = lost
        .iter()
        .find(|(id, _)| id.starts_with("search_commit:"))
        .unwrap()
        .1;
    assert_eq!(
        commit.expected,
        Expected::Exactly {
            state: eval_core::EffectState::Applied
        }
    );
    let ack = lost
        .iter()
        .find(|(id, _)| id.starts_with("search_ack:"))
        .unwrap()
        .1;
    assert_eq!(
        ack.expected,
        Expected::Exactly {
            state: eval_core::EffectState::Applied
        }
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
    for cut in ["reconciling", "reconciliation_read"] {
        assert_eq!(
            run.report.coverage.receipted.get(cut),
            Some(&2),
            "each publication episode's observer saw {cut}"
        );
    }
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
    assert_eq!(
        episode.action,
        FaultAction::ExpectedRefusal {
            refusal: ExpectedRefusal::R11DeletionBearingCatchUp
        },
        "a plain deletion provokes R11; no deletion fault is injected"
    );
    assert_eq!(episode.heal, Heal::Reopen);
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
    assert_eq!(
        episode.action,
        FaultAction::ExpectedRefusal {
            refusal: ExpectedRefusal::R24ReceiptQuotaExhausted
        },
        "a planted receipt charge provokes R24; no lock is held"
    );
    assert_eq!(episode.heal, Heal::Permanent);
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
    assert_eq!(artifact.len(), 6, "{artifact:?}");
    assert_eq!(run.report.coverage.receipted["artifact_fault_named"], 6);
    assert_eq!(
        run.report.coverage.receipted.get("ingestion_latched"),
        Some(&5),
        "a plain ingest is refused after every EIO and before its reopen"
    );
    let heals: Vec<Heal> = artifact.iter().map(|e| e.heal).collect();
    assert_eq!(
        heals.iter().filter(|h| **h == Heal::Reopen).count(),
        5,
        "every EIO latches CAS ingestion closed until reopen: {heals:?}"
    );
    assert_eq!(
        heals.iter().filter(|h| **h == Heal::Consumed).count(),
        1,
        "ENOSPC is consumed: {heals:?}"
    );
}

const SCENARIOS: [fn(&Campaign); 7] = [
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
    ]
    .into_iter()
    .collect();
    let missing: BTreeSet<&str> = owned.difference(&fired).copied().collect();
    assert_eq!(
        missing, pending,
        "every marker this suite owns fires here except the kill and liveness markers, which their scenarios record"
    );
}

fn quota_episode_on_a_fresh_root() -> (tempfile::TempDir, Witness) {
    let root = tempfile::tempdir().unwrap();
    let mut witness = Witness::new();
    fault::quota_episode(root.path(), &mut witness, 1, QUOTA_NOW_MS).unwrap();
    (root, witness)
}

#[test]
fn the_receipt_quota_refusal_outlives_every_released_allowance() {
    let (root, _) = quota_episode_on_a_fresh_root();
    let store = MemoryStore::open(&daemon::store_descriptor_in(root.path())).unwrap();
    let open: Vec<String> = store
        .with_fenced_conn_for_test(|conn| {
            conn.prepare(
                "SELECT causal_identity FROM memory_reviewer_jobs WHERE state <> 'terminal'",
            )?
            .query_map([], |row| row.get(0))?
            .collect()
        })
        .unwrap();
    for identity in open {
        store
            .finish_memory_reviewer_job(
                PROJECT,
                &identity,
                MemoryReviewerJobOutcome::Failed,
                QUOTA_NOW_MS,
            )
            .unwrap();
    }
    assert_eq!(
        store
            .memory_reviewer_headroom(PROJECT)
            .unwrap()
            .pending_jobs,
        0
    );
    let again = store.reserve_memory_reviewer_job(
        PROJECT,
        &fault::reviewer_producer("f3"),
        &fault::reviewer_inputs("cand-3"),
        QUOTA_NOW_MS,
    );
    assert!(
        matches!(
            again,
            Err(MemoryReviewerJobError::Refused(
                MemoryReviewerJobRefusal::MetadataQuota
            ))
        ),
        "R24 names a permanent refusal, not one a released allowance clears: {again:?}"
    );
}

#[test]
fn the_receipt_quota_episode_claims_no_projection_safety_check() {
    let (_root, witness) = quota_episode_on_a_fresh_root();
    assert_eq!(
        witness.safety_checks, 0,
        "the quota episode runs on a memory store of its own; no projection check ran"
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
