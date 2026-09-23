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
    Approval, ArtifactDeletionFaultKind, Coverage, Cut, CutOutcome, EffectLedger, EffectOutcome,
    EffectState, ExecutionMode, Expected, ExpectedRefusal, FaultAction, FaultReport, Heal, MARKERS,
    ProfileError, Scale, SearchEpisodeFault, StoreFamily, parse_fault_report, parse_manifest,
};
use fault::{
    Config, MANIFEST_FILE, REPORT_FILE, Run, RunError, Witness, check_expectations,
    declare_lost_reply_episode, lost_reply_episode, read_back, receipt_lost_reply_episode,
};
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
    report.validate(&run.profile).unwrap();
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
    // An armed window the runner can check from: a lock held, a latch or
    // stall that holds until the reopen, or a catch-up observer cut inside a
    // one-shot fault. ENOSPC is consumed inside its call, the corrupted copy
    // is refused before any store opens, R24 runs on a memory store, and the
    // publisher forbids its observer to call the stores before a release
    // event, by which time its fault is consumed.
    let armed = report
        .episodes
        .iter()
        .filter(|e| {
            e.scope.store != StoreFamily::Memory
                && e.action != FaultAction::CorruptQuiescentFile
                && e.action
                    != FaultAction::ArtifactDeletion {
                        fault: ArtifactDeletionFaultKind::IntentStorageExhausted,
                    }
                && !matches!(e.action, FaultAction::EmbeddingPublication { .. })
        })
        .count();
    assert!(
        report.safety_checks_while_armed >= armed as u64,
        "a safety check ran while every fault that arms on the aging drive's stores was armed: {} < {armed}",
        report.safety_checks_while_armed
    );
    assert!(report.liveness.is_none(), "liveness is a separate mode");

    let published = serde_json::from_slice(&std::fs::read(out.join(REPORT_FILE)).unwrap()).unwrap();
    assert_eq!(
        parse_fault_report(&published, &run.profile).unwrap(),
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
    let lost: Vec<_> = effects.iter().filter(|(_, e)| e.reply_lost()).collect();
    assert_eq!(lost.len(), 3, "{effects:?}");
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
    assert_eq!(
        embeddings.len(),
        1,
        "only the committed-then-lost publication loses its reply; the rolled-back one is known"
    );
    assert_eq!(
        embeddings[0].1.outcome,
        EffectOutcome::Applied,
        "a committed-then-lost reply reads back applied"
    );
    assert!(
        effects
            .keys()
            .filter(|id| id.starts_with("embedding:"))
            .count()
            == 1,
        "the rolled-back publication enters no ledger entry: {effects:?}"
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

fn quota_episode_on_a_fresh_root() -> (tempfile::TempDir, Witness, campaign::Charges) {
    let root = tempfile::tempdir().unwrap();
    let mut witness = Witness::new();
    let profile = fault::profile(Scale::S0, MESSAGES, 600_000, None);
    let mut charges = campaign::Charges::new(profile.envelope);
    fault::quota_episode(root.path(), &mut witness, &mut charges, 1, QUOTA_NOW_MS).unwrap();
    (root, witness, charges)
}

/// The R24 store's footprint is charged while its connection is open; the
/// drop that ends the episode can checkpoint the WAL away.
#[test]
fn the_receipt_quota_episode_charges_its_store_while_open() {
    let (root, _, charges) = quota_episode_on_a_fresh_root();
    let closed = campaign::root_bytes(root.path());
    let peak = charges.envelope.peaks.store_bytes;
    assert!(
        peak > 0 && peak >= closed,
        "the open footprint is charged: peak {peak}, closed {closed}"
    );
}

#[test]
fn the_receipt_quota_refusal_outlives_every_released_allowance() {
    let (root, _, _) = quota_episode_on_a_fresh_root();
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
    let (_root, witness, _) = quota_episode_on_a_fresh_root();
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
            assert!(effect.reply_lost() && effect.attempted == 1, "{effect:?}");
        }
    }
}

#[test]
fn a_lost_reply_without_a_matching_fixed_expectation_refuses_the_run() {
    let identity = "search_commit:9";
    let mut effects = EffectLedger::default();
    effects.attempt(identity);
    effects.lose_reply(identity, "episode").unwrap();
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
    // A read-back fixes the expectation, so the applied case is a second
    // ledger, not a second read-back of the same effect.
    let mut applied = EffectLedger::default();
    applied.attempt(identity);
    applied.lose_reply(identity, "episode").unwrap();
    applied.read_back(identity, EffectState::Applied).unwrap();
    check_expectations(&fixed, &applied).unwrap();
    let stray = BTreeMap::from([("search_ack:9".to_string(), EffectState::Applied)]);
    assert!(
        check_expectations(&stray, &applied).is_err(),
        "an expectation for an effect the campaign never lost"
    );
}

#[test]
fn a_read_back_after_later_catch_up_is_refused_as_masked() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = aging::Stores::open(root.path(), &plan);
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

/// `--messages 7` plans a history whose checkpoint leaves fewer than the six
/// suffix steps the fault phase drives. The run refuses it as a `RunError`
/// before any store opens; it does not panic on accepted numeric input.
#[test]
fn a_history_too_short_for_the_fault_phase_is_refused_not_panicked() {
    let plan = aging::plan(7).unwrap();
    assert!(
        plan.steps.len() - (plan.checkpoint_step as usize) < 6,
        "the case needs a short suffix: {} steps, checkpoint {}",
        plan.steps.len(),
        plan.checkpoint_step
    );
    let publish = tempfile::tempdir().unwrap();
    let mut config = config(publish.path().join("out"), 600_000);
    config.messages = 7;
    let outcome = std::panic::catch_unwind(|| fault::run(&config).err());
    let refused = outcome
        .expect("a short history is refused, not a panic")
        .expect("a short history is refused");
    assert!(
        refused.to_string().contains("after the checkpoint"),
        "{refused}"
    );
    assert!(
        !publish.path().join("out").join(REPORT_FILE).exists(),
        "nothing is published"
    );
}

/// The publication probes apply steps until a publish opens an embedding job,
/// and recovery needs a step left to live; a history the probes would exhaust
/// is refused by the same planning check, before any store opens, rather
/// than by the campaign after it has opened and mutated its stores.
#[test]
fn a_history_the_publication_probes_would_exhaust_is_refused_before_any_store_opens() {
    let plan = aging::plan(9).unwrap();
    assert!(
        plan.steps.len() - (plan.checkpoint_step as usize) >= 6,
        "the case needs a suffix the six-step check accepts: {} steps, checkpoint {}",
        plan.steps.len(),
        plan.checkpoint_step
    );
    let publish = tempfile::tempdir().unwrap();
    let mut config = config(publish.path().join("out"), 600_000);
    config.messages = 9;
    let refused = fault::run(&config).err().expect("the history is refused");
    assert!(
        matches!(refused, RunError::HistoryTooShort { .. }),
        "refused at planning, not by the campaign: {refused}"
    );
    assert!(
        !publish.path().join("out").join(REPORT_FILE).exists(),
        "nothing is published"
    );
}

/// The campaign's store-byte peak is the open footprint: closing the stores
/// checkpoints every WAL away, so a peak read only after the close would
/// let a run pass its bound while exceeding it.
#[test]
fn the_campaign_charges_the_stores_at_their_open_footprint_not_after_the_close() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = aging::Stores::open(root.path(), &plan);
    aging::live(&mut stores, &plan.steps);
    let open = campaign::root_bytes(root.path());
    drop(stores.close());
    let closed = campaign::root_bytes(root.path());
    assert!(
        open > closed,
        "closing checkpoints the WAL away: {open} vs {closed}"
    );
    let profile = fault::profile(Scale::S0, MESSAGES, 600_000, None);
    let mut charges = campaign::Charges::new(profile.envelope);
    let mut witness = Witness::new();
    fault::campaign(&plan, &mut charges, &mut witness).unwrap();
    let peak = charges.envelope.peaks.store_bytes;
    assert!(
        peak > (open + closed) / 2,
        "the peak charged is the open footprint: peak {peak}, open {open}, closed {closed}"
    );
}

/// Each recovery closes the stores, which checkpoints the WALs away, so the
/// footprint before a recovery is charged too, not only the one at the end.
/// With ten messages the run's largest footprint is the one before the first
/// recovery; a peak charged only at the end would understate it.
#[test]
fn the_campaign_charges_the_stores_before_each_recovery_closes_them() {
    let plan = fault::plan(10).unwrap();
    let k = plan.checkpoint_step as usize;
    let steps = &plan.steps[k..];
    // The footprint the campaign reaches right before its first recovery,
    // measured on a root of its own by the same steps.
    let root = tempfile::tempdir().unwrap();
    let mut stores = aging::Stores::open(root.path(), &plan);
    aging::live(&mut stores, &plan.steps[..k]);
    let mut witness = Witness::new();
    stores.apply(&steps[0]);
    fault::lock_holder_episode(&mut stores, &mut witness, "lock", k as u32, steps[0].now_ms)
        .unwrap();
    stores.drain(steps[0].now_ms);
    for planned in &steps[1..3] {
        stores.apply(planned);
        stores.drain(planned.now_ms);
    }
    stores.apply(&steps[3]);
    lost_reply_episode(
        &mut stores,
        &mut witness,
        "lost",
        (k + 3) as u32,
        steps[3].now_ms,
        SearchEpisodeFault::LoseLocalCommitReply,
    )
    .unwrap();
    let before_recovery = campaign::root_bytes(root.path());
    let closed = stores.close();
    let mut stores = closed.reopen(steps[4].now_ms);
    aging::live(&mut stores, &steps[4..]);
    let end_open = campaign::root_bytes(root.path());
    drop(stores.close());
    assert!(
        before_recovery > end_open,
        "the case needs its peak before the recovery: {before_recovery} vs {end_open}"
    );

    let profile = fault::profile(Scale::S0, 10, 600_000, None);
    let mut charges = campaign::Charges::new(profile.envelope);
    let mut witness = Witness::new();
    fault::campaign(&plan, &mut charges, &mut witness).unwrap();
    let peak = charges.envelope.peaks.store_bytes;
    assert!(
        peak > (before_recovery + end_open) / 2,
        "the peak charged is the footprint before the recovery: peak {peak}, before {before_recovery}, end {end_open}"
    );
}

/// Every CAS episode's reopen closes the stores, which checkpoints the WALs
/// away, so the footprint is charged before each of those closes too, as the
/// recoveries charge it before theirs.
#[test]
fn the_cas_episodes_charge_the_stores_before_each_reopen_closes_them() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = aging::Stores::open(root.path(), &plan);
    aging::live(&mut stores, &plan.steps[..3]);
    let open = campaign::root_bytes(root.path());
    let profile = fault::profile(Scale::S0, MESSAGES, 600_000, None);
    let mut charges = campaign::Charges::new(profile.envelope);
    let mut witness = Witness::new();
    let (stores, _) = fault::artifact_ingest_episodes(
        stores,
        &mut witness,
        &mut charges,
        3,
        plan.steps[3].now_ms,
    )
    .unwrap();
    drop(stores.close());
    let peak = charges.envelope.peaks.store_bytes;
    assert!(
        peak >= open,
        "the footprint before each CAS reopen is charged: peak {peak}, open before the episodes {open}"
    );
}

/// `safety_checks_while_armed` counts checks made while a fault is armed. A
/// reply-loss fault is consumed when its episode returns, so the checks after
/// the episode and after the reopen run as assertions but are not counted;
/// the episode's own check runs at the cut where the fault is armed.
#[test]
fn a_safety_check_outside_an_armed_window_is_not_counted_as_armed() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = aging::Stores::open(root.path(), &plan);
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
    let armed = witness.safety_checks;
    assert!(
        armed >= 1,
        "the episode checks safety while its fault is armed"
    );
    witness.safety_check(&stores);
    assert_eq!(
        witness.safety_checks, armed,
        "a check after the episode returned is not a check while armed"
    );
}
