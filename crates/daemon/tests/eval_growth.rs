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
    Approval, Coverage, EnvelopeExceeded, ExpectedRefusal, GrowthContract, GrowthMode,
    GrowthRefused, GrowthReport, MARKERS, Operation, ProfileError, Resource, Scale, StoreFamily,
    digests_match_serial, isolated, parse_growth_report, parse_manifest,
};
use growth::{Campaign, Config, MANIFEST_FILE, REPORT_FILE, Run, RunError};
use memory_store::memory_reviewer_jobs::{
    MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT, MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT,
    MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES, MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES,
};
use support::memory_reviewer_publish::{PROJECT, now_ms};

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
        store_bytes_bound: None,
    }
}

struct Published {
    run: Run,
    config: Config,
    out: PathBuf,
    _publish: tempfile::TempDir,
}

fn campaign(mode: GrowthMode, coverage: &mut Coverage) -> Published {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let config = config(out.clone(), budget().unwrap_or(600_000), mode);
    let run = growth::run(&config).unwrap();
    for marker in run.coverage.fired() {
        coverage.record(marker).unwrap();
    }
    Published {
        run,
        config,
        out,
        _publish: publish,
    }
}

/// What the test holds the report to, derived from the config it ran and the
/// store's constants, not read back from the report.
fn contract(published: &Published) -> GrowthContract {
    let config = &published.config;
    let profile = growth::profile(
        config.scale,
        config.messages,
        config.elapsed_bound_ms,
        config.approval.clone(),
    );
    GrowthContract {
        quota: growth::quota(),
        bounds: growth::bounds(&profile, config.messages),
        envelope: profile.envelope.clone(),
    }
}

fn a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore_scenario(
    published: &Published,
) {
    let report = &published.run.report;
    let contract = contract(published);
    report.validate(&contract).unwrap();
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
    assert_eq!(
        report.envelope.peaks.store_bytes,
        report.ledger.peak_store_bytes(),
        "only the samples charged store bytes, with the transient total each saw"
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
        &contract,
    )
    .unwrap();
    assert_eq!(parsed, *report);
    let manifest = parse_manifest(
        &serde_json::from_slice(&std::fs::read(published.out.join(MANIFEST_FILE)).unwrap())
            .unwrap(),
    )
    .unwrap();
    let value = serde_json::to_value(report.serialize(&contract).unwrap()).unwrap();
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
    let tip = live.tip();
    assert_eq!(
        charges.envelope.peaks.store_bytes,
        live.ledger.peak_store_bytes(),
        "only the samples charged the envelope, with the transient total each saw"
    );
    let (live, refused) = live.restore(1_700_000_000_000);
    assert!(
        matches!(
            refused,
            Err(RunError::Growth(GrowthRefused::RestoreUnderNeverRestored))
        ),
        "{refused:?}"
    );
    assert_eq!(live.ledger.restores_refused, 1);
    assert_eq!(live.tip(), tip);
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
            Some(sample.headroom.project_metadata_bytes),
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
    let ledger = live.finish(&mut charges).unwrap().ledger;
    assert!(matches!(
        ledger.verdict(
            &growth::quota(),
            &growth::bounds(&growth::profile(Scale::S0, 128, 600_000, None), MESSAGES)
        ),
        Err(GrowthRefused::NotALeakVerdict { .. })
    ));
}

#[test]
fn the_final_sample_reads_the_closed_files_without_reopening_the_memory_store() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut charges = Charges::new(growth::profile(Scale::S0, 128, 600_000, None).envelope);
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    for _ in 0..4 {
        live.step(&mut charges).unwrap();
    }
    // Every open of the memory store commits a higher fence epoch, so an
    // unchanged epoch shows nothing opened it between the close and the sample.
    let memory = aging::memory_file(root.path());
    let fence_epoch = || {
        aging::read_only(&memory)
            .query_row("SELECT epoch FROM fence WHERE id = 0", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    let epoch = fence_epoch();
    let headroom = live.headroom();
    assert!(headroom.admitted_total > 0);
    let finished = live.finish(&mut charges).unwrap();
    assert_eq!(
        fence_epoch(),
        epoch,
        "the final sample reopened the memory store"
    );
    let last = finished.ledger.samples.last().unwrap();
    assert_eq!(last.headroom, headroom);
    assert_eq!(
        last.stores[&StoreFamily::Memory].file,
        std::fs::metadata(&memory).unwrap().len()
    );
}

#[test]
fn releasing_the_campaign_root_charges_no_store_bytes_beyond_the_samples() {
    let mut charges = Charges::new(growth::profile(Scale::S0, 128, 600_000, None).envelope);
    let root = charges.occupy().unwrap();
    // Artifact objects are charged as artifact bytes at every sample; a
    // whole-root walk at release would charge them as store bytes too.
    let objects = root.path().join("kernel/artifacts/objects");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join("object"), vec![0u8; 1 << 20]).unwrap();
    charges.observe(Resource::StoreBytes, 4096).unwrap();
    charges.release(root).unwrap();
    assert_eq!(charges.envelope.peaks.store_bytes, 4096);
    assert_eq!(charges.roots(), 0);
}

#[test]
fn quota_pressure_past_the_pending_cap_settles_new_admissions_instead_of_panicking() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    // Every other admission stays pending, so twice the cap overruns it.
    let admissions = 2 * MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT + 4;
    for k in 0..admissions {
        live.quota_pressure(4 * k + 3, now_ms()).unwrap();
    }
    let headroom = live.headroom();
    assert_eq!(
        headroom.pending_jobs,
        MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT as u64 - 1,
        "the pending queue stops one short of the store's cap"
    );
    assert_eq!(headroom.admitted_total, admissions as u64);
    assert_eq!(
        headroom.pending_jobs + headroom.terminal_jobs,
        headroom.admitted_total
    );
    assert_eq!(headroom.r24_refusals, 0);
    assert_eq!(
        Some(headroom.project_metadata_bytes),
        growth::quota().expected_project_bytes(&headroom)
    );
}

#[test]
fn a_receipt_quota_refusal_is_counted_as_r24_and_admits_nothing() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    // Step 3 settles its admission, so the one row holds a receipt charge only.
    live.quota_pressure(3, now_ms()).unwrap();
    let over_quota = MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT
        - (MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES + MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES)
        + 1;
    live.stores
        .memory
        .with_fenced_conn_for_test(|conn| {
            conn.execute(
                "UPDATE memory_reviewer_jobs SET receipt_charge_bytes = ?1",
                [i64::try_from(over_quota).unwrap()],
            )
        })
        .unwrap();
    let before = live
        .stores
        .memory
        .memory_reviewer_headroom(PROJECT)
        .unwrap();
    live.quota_pressure(7, now_ms()).unwrap();
    let after = live
        .stores
        .memory
        .memory_reviewer_headroom(PROJECT)
        .unwrap();
    assert_eq!(after, before, "a refused reservation charges nothing");
    let headroom = live.headroom();
    assert_eq!(headroom.r24_refusals, 1);
    assert_eq!(headroom.admitted_total, 1);
    assert_eq!(live.mix.counts[&Operation::QuotaPressure], 2);
    // The count the report reconciles against: one recorded refusal per R24,
    // carrying the variant as production prints it.
    let recorded: Vec<_> = live
        .witness
        .refusals
        .iter()
        .filter(|r| r.refusal == ExpectedRefusal::R24ReceiptQuotaExhausted)
        .collect();
    assert_eq!(recorded.len(), 1, "{:?}", live.witness.refusals);
    assert!(ExpectedRefusal::R24ReceiptQuotaExhausted.evidences(&recorded[0].production_error));
}

fn a_deliberate_envelope_breach_names_the_resource_and_publishes_nothing_scenario(
    coverage: &mut Coverage,
) {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let mut config = config(out.clone(), 600_000, GrowthMode::NeverRestored);
    config.store_bytes_bound = Some(64 << 10);
    let error = growth::run(&config)
        .err()
        .expect("a 64 KiB store bound is crossed");
    let RunError::Envelope(EnvelopeExceeded {
        resource,
        bound,
        observed,
    }) = error
    else {
        panic!("{error}");
    };
    assert_eq!(resource, Resource::StoreBytes);
    assert_eq!(bound, 64 << 10);
    assert!(observed > bound);
    assert!(
        !out.join(REPORT_FILE).exists() && !out.join(MANIFEST_FILE).exists(),
        "nothing is published after a breach"
    );

    // Driven step by step, the peak that crossed the bound is the one recorded.
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
    let exceeded = breach.expect("the bound is crossed within eight steps");
    assert_eq!(charges.envelope.peaks.store_bytes, exceeded.observed);
    coverage.record("xc_envelope_breach_stops_the_run").unwrap();
}

#[test]
fn a_deliberate_envelope_breach_names_the_resource_and_publishes_nothing() {
    a_deliberate_envelope_breach_names_the_resource_and_publishes_nothing_scenario(
        &mut Coverage::default(),
    );
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
    // Refused before planning too: a history the generator would refuse does
    // not get generated, or judged, for an unapproved run.
    config.messages = 0;
    assert!(aging::plan(0).is_err());
    let error = growth::run(&config).err().unwrap();
    assert!(
        matches!(error, RunError::Profile(ProfileError::NotApproved { .. })),
        "{error}"
    );
}

/// The figures cross-talk between two campaigns would move.
type Shape = (u64, u64, u64, std::collections::BTreeMap<Operation, u64>);

/// One campaign's result digest, shape, and the resources it held.
type Outcome = (String, Shape, eval_core::CampaignResources);

fn shape(run: &Run) -> Shape {
    let last = run.report.ledger.samples.last().unwrap();
    (
        last.commit_log_rows,
        last.projection_rows,
        last.artifact_objects,
        run.report.mix.counts.clone(),
    )
}

fn two_concurrent_campaigns_on_one_checkout_match_their_serial_digests_scenario(
    budget_ms: u64,
    coverage: &mut Coverage,
) {
    fn one(publish: &std::path::Path, budget_ms: u64) -> Outcome {
        let run = growth::run(&config(
            publish.join("out"),
            budget_ms,
            GrowthMode::NeverRestored,
        ))
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&run.report_bytes).unwrap();
        (
            GrowthReport::result_digest(&value).unwrap(),
            shape(&run),
            run.resources,
        )
    }
    let serial: Vec<_> = (0..2)
        .map(|_| {
            let publish = tempfile::tempdir().unwrap();
            one(publish.path(), budget_ms)
        })
        .collect();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            std::thread::spawn(move || {
                let publish = tempfile::tempdir().unwrap();
                one(publish.path(), budget_ms)
            })
        })
        .collect();
    let concurrent: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    isolated(&concurrent[0].2, &concurrent[1].2).unwrap();
    isolated(&serial[0].2, &serial[1].2).unwrap();
    digests_match_serial(
        &concurrent
            .iter()
            .map(|(d, _, _)| d.clone())
            .collect::<Vec<_>>(),
        &serial.iter().map(|(d, _, _)| d.clone()).collect::<Vec<_>>(),
    )
    .unwrap();
    for (i, (_, concurrent_shape, _)) in concurrent.iter().enumerate() {
        assert_eq!(
            concurrent_shape, &serial[i].1,
            "campaign {i}: rows, objects, and the mix are what the serial run produced"
        );
    }
    let mut shared = concurrent[1].2.clone();
    shared.roots = concurrent[0].2.roots.clone();
    assert!(matches!(
        isolated(&concurrent[0].2, &shared),
        Err(eval_core::IsolationRefused::SharedRoot { .. })
    ));
    coverage.record("xc_parallel_campaigns_isolated").unwrap();
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn two_concurrent_campaigns_on_one_checkout_match_their_serial_digests() {
    let budget_ms = budget_or_panic();
    two_concurrent_campaigns_on_one_checkout_match_their_serial_digests_scenario(
        budget_ms,
        &mut Coverage::default(),
    );
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn every_growth_marker_fires_across_the_scenarios() {
    let budget_ms = budget_or_panic();
    let mut coverage = Coverage::default();
    let published = campaign(GrowthMode::NeverRestored, &mut coverage);
    a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore_scenario(&published);
    reviewer_headroom_is_accounted_from_the_stores_own_constants_scenario(&published);
    the_swarm_mix_exercises_every_operation_kind_scenario(&published);
    a_deliberate_envelope_breach_names_the_resource_and_publishes_nothing_scenario(&mut coverage);
    two_concurrent_campaigns_on_one_checkout_match_their_serial_digests_scenario(
        budget_ms,
        &mut coverage,
    );
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

#[test]
fn the_profile_is_declared_from_the_message_count_not_the_step_count() {
    // 48 messages plan more than 64 steps, so a profile built from the step
    // count would declare a wider event bound than the generator used.
    let publish = tempfile::tempdir().unwrap();
    let mut config = config(
        publish.path().join("out"),
        600_000,
        GrowthMode::NeverRestored,
    );
    config.messages = 48;
    assert!(aging::plan(48).unwrap().steps.len() > 64);
    let run = growth::run(&config).unwrap();
    let declared = growth::profile(
        config.scale,
        config.messages,
        config.elapsed_bound_ms,
        config.approval.clone(),
    );
    assert_eq!(run.report.profile_digest, declared.digest().unwrap());
}

#[test]
fn a_fifth_step_that_commits_nothing_runs_no_lost_reply_episode() {
    let mut plan = aging::plan(MESSAGES).unwrap();
    // The same retire twice in a row: the second finds its object already
    // dead (or still unpublished) and commits nothing, on the step that would
    // otherwise lose an acknowledgement reply.
    let retire = plan
        .steps
        .iter()
        .find(|p| matches!(p.step, aging::Step::Retire(_)))
        .unwrap()
        .step
        .clone();
    plan.steps[3].step = retire.clone();
    plan.steps[4].step = retire;
    let root = tempfile::tempdir().unwrap();
    let mut charges = Charges::new(growth::profile(Scale::S0, 128, 600_000, None).envelope);
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    for _ in 0..5 {
        live.step(&mut charges).unwrap();
    }
    assert_eq!(live.mix.counts.get(&Operation::FaultEpisode), None);
}

#[test]
fn a_symlink_under_the_artifact_objects_is_neither_an_object_nor_its_bytes() {
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut charges = Charges::new(growth::profile(Scale::S0, 128, 600_000, None).envelope);
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    for _ in 0..4 {
        live.step(&mut charges).unwrap();
    }
    let before = live.ledger.samples.last().unwrap().clone();
    assert!(before.artifact_objects > 0);
    let objects = root.path().join("kernel").join("artifacts").join("objects");
    std::os::unix::fs::symlink(aging::kernel_file(root.path()), objects.join("link")).unwrap();
    live.sample(before.step + 1, &mut charges).unwrap();
    let after = live.ledger.samples.last().unwrap();
    assert_eq!(after.artifact_objects, before.artifact_objects);
    assert_eq!(after.artifact_bytes, before.artifact_bytes);
}

#[test]
fn a_manifest_the_directory_refuses_takes_the_report_back_out() {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let config = config(out.clone(), 600_000, GrowthMode::NeverRestored);
    // Plant the manifest's name once the run has prepared its directory and
    // is driving the campaign, which takes seconds; the report then publishes
    // and the manifest cannot follow it.
    let planted = out.join(MANIFEST_FILE);
    let planter = std::thread::spawn({
        let out = out.clone();
        let planted = planted.clone();
        move || {
            while !out.is_dir() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::fs::write(&planted, b"not a manifest").unwrap();
        }
    });
    let error = growth::run(&config).err().expect("the manifest is refused");
    planter.join().unwrap();
    let RunError::Publish { path, kind } = error else {
        panic!("{error}");
    };
    assert_eq!(path, planted);
    assert_eq!(kind, std::io::ErrorKind::AlreadyExists);
    assert!(
        !out.join(REPORT_FILE).exists(),
        "a reader finds both files or none"
    );
}

#[test]
fn the_artifact_store_is_bounded_by_the_ledger_not_charged_to_the_envelope() {
    // The envelope's artifact bytes are the published files; the artifact
    // store a sample measures is judged by `GrowthBounds::artifact_bytes`.
    let plan = aging::plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut profile = growth::profile(Scale::S0, 128, 600_000, None);
    profile.envelope.artifact_bytes = 1;
    let mut charges = Charges::new(profile.envelope);
    let mut live = Campaign::open(root.path(), plan, GrowthMode::NeverRestored);
    for _ in 0..4 {
        live.step(&mut charges).unwrap();
    }
    assert!(live.ledger.samples.last().unwrap().artifact_bytes > 1);
    assert_eq!(charges.envelope.peaks.artifact_bytes, 0);
}
