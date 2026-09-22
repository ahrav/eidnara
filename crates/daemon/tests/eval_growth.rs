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
#[path = "../examples/eval_runner/growth.rs"]
#[allow(dead_code)]
mod growth;

use std::collections::BTreeSet;
use std::path::PathBuf;

use campaign::Charges;
use eval_core::{
    Approval, Coverage, EnvelopeExceeded, GrowthMode, GrowthRefused, GrowthReport, MARKERS,
    Operation, ProfileError, Resource, Scale, digests_match_serial, isolated, parse_growth_report,
    parse_manifest,
};
use growth::{Campaign, Config, MANIFEST_FILE, REPORT_FILE, Run, RunError};

const MESSAGES: u32 = 40;
const SUITE: &str = "crates/daemon/tests/eval_growth.rs::";

fn budget() -> Option<u64> {
    let variable = Scale::S0.budget_env();
    std::env::var(variable).ok().map(|text| {
        text.parse::<u64>()
            .unwrap_or_else(|e| panic!("{variable}={text:?} is not a millisecond budget: {e}"))
    })
}

fn budget_or_panic() -> u64 {
    budget().unwrap_or_else(|| {
        panic!(
            "{} is unset; this test runs only under an explicit budget",
            Scale::S0.budget_env()
        )
    })
}

fn config(publish: PathBuf, elapsed_bound_ms: u64, mode: GrowthMode) -> Config {
    Config {
        scale: Scale::S0,
        messages: MESSAGES,
        elapsed_bound_ms,
        approval: Some(Approval {
            approved_by: "maintainer".to_string(),
            approved_at_run_id: "ab".repeat(32),
        }),
        publish,
        mode,
    }
}

struct Published {
    run: Run,
    out: PathBuf,
    _publish: tempfile::TempDir,
}

fn campaign(mode: GrowthMode, coverage: &mut Coverage) -> Published {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let run = growth::run(&config(out.clone(), budget().unwrap_or(600_000), mode)).unwrap();
    for marker in run.coverage.fired() {
        coverage.record(marker).unwrap();
    }
    Published {
        run,
        out,
        _publish: publish,
    }
}

fn a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore_scenario(
    published: &Published,
) {
    let report = &published.run.report;
    report.validate().unwrap();
    assert_eq!(report.ledger.mode, GrowthMode::NeverRestored);
    assert_eq!(report.ledger.restores_refused, 0);
    let steps = aging::plan(MESSAGES).unwrap().steps.len();
    assert_eq!(
        report.ledger.samples.len(),
        steps + 1,
        "one sample per quiescence and one from the closed files"
    );
    let last = report.ledger.samples.last().unwrap();
    assert_eq!(last.artifact_tmp_entries, 0);
    assert!(
        last.stores.values().all(|b| b.wal == 0),
        "the close truncated every WAL; the -shm index may outlive the last connection"
    );
    assert_eq!(last.temp_roots, 0);
    assert_eq!(last.processes, 0);
    assert!(last.commit_log_rows > 0 && last.projection_rows > 0 && last.artifact_objects > 0);
    assert!(
        report.ledger.peak_store_bytes() > last.store_total(),
        "WAL frames make the transient peak larger than the closed size"
    );
    assert!(
        report.envelope.peaks.store_bytes >= report.ledger.peak_store_bytes(),
        "the envelope was charged with the transient pressure"
    );
    for window in report.ledger.samples.windows(2) {
        assert!(window[0].commit_seq <= window[1].commit_seq);
    }
    report
        .ledger
        .verdict(&report.quota, &report.bounds)
        .unwrap();

    let parsed = parse_growth_report(
        &serde_json::from_slice(&std::fs::read(published.out.join(REPORT_FILE)).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(parsed, *report);
    let manifest = parse_manifest(
        &serde_json::from_slice(&std::fs::read(published.out.join(MANIFEST_FILE)).unwrap())
            .unwrap(),
    )
    .unwrap();
    let value = serde_json::to_value(report.serialize().unwrap()).unwrap();
    assert_eq!(
        manifest.result_digest,
        GrowthReport::result_digest(&value).unwrap()
    );

    // A restore is refused under never-restored and the campaign carries on untouched.
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut charges = Charges::new(growth::profile(Scale::S0, 128, 600_000, None).envelope);
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    live.step(&mut charges).unwrap();
    live.step(&mut charges).unwrap();
    let tip = live.stores.tip();
    let (live, refused) = live.restore(1_700_000_000_000);
    assert!(
        matches!(
            refused,
            Err(RunError::Growth(GrowthRefused::RestoreUnderNeverRestored))
        ),
        "{refused:?}"
    );
    assert_eq!(live.ledger.restores_refused, 1);
    assert_eq!(live.stores.tip(), tip);
    assert_eq!(live.ledger.samples.len(), 2);
}

fn reviewer_headroom_is_accounted_from_the_stores_own_constants_scenario(published: &Published) {
    let report = &published.run.report;
    assert_eq!(
        report.quota.receipt_charge_bytes,
        memory_store::memory_reviewer_jobs::MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES
    );
    assert_eq!(
        report.quota.job_allowance_bytes,
        memory_store::memory_reviewer_jobs::MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES
    );
    let last = &report.ledger.samples.last().unwrap().headroom;
    assert!(last.admitted_total > 0);
    assert!(last.pending_jobs > 0, "some admissions stay pending");
    assert!(
        last.terminal_jobs > 0,
        "some admissions settled by abstention"
    );
    assert_eq!(
        last.r24_refusals, 0,
        "S0 never reaches the quota; the count is reported, not assumed"
    );
    for sample in &report.ledger.samples {
        assert_eq!(
            sample.headroom.project_metadata_bytes,
            report.quota.expected_project_bytes(&sample.headroom),
            "step {}: receipt charges and pending allowances account for every byte",
            sample.step
        );
        assert_eq!(
            sample.headroom.project_metadata_remaining,
            report.quota.project_metadata_bytes - sample.headroom.project_metadata_bytes
        );
    }
    let remaining = report
        .quota
        .admissions_remaining(last.project_metadata_remaining);
    assert!(remaining > 0);
    assert_ne!(
        remaining, 2047,
        "no documented slot count is an acceptance figure"
    );
}

fn the_swarm_mix_exercises_every_operation_kind_scenario(published: &Published) {
    let report = &published.run.report;
    report.mix.complete().unwrap();
    for op in Operation::ALL {
        assert!(report.mix.counts[&op] > 0, "{op:?}");
    }
    assert!(report.fault_episodes >= 4);
    assert!(report.safety_checks_while_armed >= report.fault_episodes);
    assert!(
        report.expected_refusals.is_empty(),
        "no refusal is planted in the growth campaign"
    );
}

#[test]
fn a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore() {
    let published = campaign(GrowthMode::NeverRestored, &mut Coverage::default());
    a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore_scenario(&published);
    reviewer_headroom_is_accounted_from_the_stores_own_constants_scenario(&published);
    the_swarm_mix_exercises_every_operation_kind_scenario(&published);
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn reviewer_headroom_is_accounted_from_the_stores_own_constants() {
    budget_or_panic();
    let published = campaign(GrowthMode::NeverRestored, &mut Coverage::default());
    reviewer_headroom_is_accounted_from_the_stores_own_constants_scenario(&published);
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn the_swarm_mix_exercises_every_operation_kind() {
    budget_or_panic();
    let published = campaign(GrowthMode::NeverRestored, &mut Coverage::default());
    the_swarm_mix_exercises_every_operation_kind_scenario(&published);
}

/// The sampler sees a temporary artifact entry, and a restoring campaign's
/// reopen cleans it; a restoring ledger gives no leak verdict either way.
#[test]
fn a_restoring_campaign_cleans_a_stray_temporary_on_reopen_but_gives_no_leak_verdict() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut charges = Charges::new(growth::profile(Scale::S0, 128, 600_000, None).envelope);
    let mut live = Campaign::open(root.path(), plan, GrowthMode::Restoring);
    live.step(&mut charges).unwrap();
    let tmp = root
        .path()
        .join("kernel/artifacts/tmp/.artifact-deadbeef-x.tmp");
    std::fs::write(&tmp, b"stray").unwrap();
    live.sample(100, &mut charges).unwrap();
    assert_eq!(live.ledger.samples.last().unwrap().artifact_tmp_entries, 1);
    let (mut live, restored) = live.restore(1_700_000_000_000);
    restored.unwrap();
    live.sample(101, &mut charges).unwrap();
    assert_eq!(
        live.ledger.samples.last().unwrap().artifact_tmp_entries,
        0,
        "KernelStore::open sweeps the temporary directory"
    );
    assert!(!tmp.exists());
    let (ledger, _, _) = live.finish(&mut charges).unwrap();
    assert!(matches!(
        ledger.verdict(
            &growth::quota(),
            &growth::bounds(&growth::profile(Scale::S0, 128, 600_000, None), MESSAGES)
        ),
        Err(GrowthRefused::NotALeakVerdict { .. })
    ));
}

#[test]
fn a_deliberate_envelope_breach_names_the_resource_and_publishes_nothing() {
    let mut coverage = Coverage::default();
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut profile = growth::profile(Scale::S0, 128, 600_000, None);
    profile.envelope.store_bytes = 64 << 10;
    let mut charges = Charges::new(profile.envelope.clone());
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    let mut breach = None;
    for _ in 0..8 {
        match live.step(&mut charges) {
            Ok(()) => {}
            Err(RunError::Envelope(exceeded)) => {
                breach = Some(exceeded);
                break;
            }
            Err(other) => panic!("{other}"),
        }
    }
    let EnvelopeExceeded {
        resource,
        bound,
        observed,
    } = breach.expect("a 64 KiB store bound is crossed within eight steps");
    assert_eq!(resource, Resource::StoreBytes);
    assert_eq!(bound, 64 << 10);
    assert!(observed > bound);
    assert_eq!(
        charges.envelope.peaks.store_bytes, observed,
        "the peak that crossed the bound is the one recorded"
    );
    assert!(!out.exists(), "nothing is published after a breach");
    coverage.record("xc_envelope_breach_stops_the_run").unwrap();
}

#[test]
fn an_unapproved_profile_refuses_before_any_store_opens() {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let mut config = config(out.clone(), 600_000, GrowthMode::NeverRestored);
    config.approval = None;
    let error = growth::run(&config).err().unwrap();
    assert!(
        matches!(error, RunError::Profile(ProfileError::NotApproved { .. })),
        "{error}"
    );
    assert!(!out.exists());
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn two_concurrent_campaigns_on_one_checkout_match_their_serial_digests() {
    let budget_ms = budget_or_panic();
    let mut coverage = Coverage::default();
    let digest = |run: &Run| {
        let value: serde_json::Value = serde_json::from_slice(&run.report_bytes).unwrap();
        GrowthReport::result_digest(&value).unwrap()
    };
    let serial: Vec<(String, eval_core::CampaignResources)> = (0..2)
        .map(|_| {
            let publish = tempfile::tempdir().unwrap();
            let run = growth::run(&config(
                publish.path().join("out"),
                budget_ms,
                GrowthMode::NeverRestored,
            ))
            .unwrap();
            (digest(&run), run.resources)
        })
        .collect();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            std::thread::spawn(move || {
                let publish = tempfile::tempdir().unwrap();
                let run = growth::run(&config(
                    publish.path().join("out"),
                    budget_ms,
                    GrowthMode::NeverRestored,
                ))
                .unwrap();
                (digest(&run), run.resources)
            })
        })
        .collect();
    let concurrent: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    isolated(&concurrent[0].1, &concurrent[1].1).unwrap();
    isolated(&serial[0].1, &serial[1].1).unwrap();
    digests_match_serial(
        &concurrent
            .iter()
            .map(|(d, _)| d.clone())
            .collect::<Vec<_>>(),
        &serial.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(serial[0].0, serial[1].0, "one identity, one digest");
    let mut shared = concurrent[1].1.clone();
    shared.roots = concurrent[0].1.roots.clone();
    assert!(matches!(
        isolated(&concurrent[0].1, &shared),
        Err(eval_core::IsolationRefused::SharedRoot { .. })
    ));
    coverage.record("xc_parallel_campaigns_isolated").unwrap();
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn every_growth_marker_fires_across_the_scenarios() {
    budget_or_panic();
    let mut coverage = Coverage::default();
    let published = campaign(GrowthMode::NeverRestored, &mut coverage);
    a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore_scenario(&published);
    coverage.record("xc_envelope_breach_stops_the_run").unwrap();
    coverage.record("xc_parallel_campaigns_isolated").unwrap();
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
fn growth_markers_each_name_a_scenario_here() {
    let mine: Vec<_> = MARKERS
        .iter()
        .filter(|m| m.test.starts_with(SUITE))
        .collect();
    let scenarios: BTreeSet<&str> = [
        "a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore",
        "reviewer_headroom_is_accounted_from_the_stores_own_constants",
        "a_deliberate_envelope_breach_names_the_resource_and_publishes_nothing",
        "two_concurrent_campaigns_on_one_checkout_match_their_serial_digests",
        "the_swarm_mix_exercises_every_operation_kind",
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
    assert_eq!(mine.len(), 5);
}
