use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    CampaignResources, ClaimBoundary, Coverage, Envelope, GROWTH_REPORT_SCHEMA, GrowthBounds,
    GrowthLedger, GrowthMode, GrowthRefused, GrowthReport, GrowthReportError, HeadroomSample,
    IsolationRefused, MixIncomplete, Operation, ResourceLimits, ResourceSample, ReviewerQuota,
    StoreBytes, StoreFamily, SwarmMix, digests_match_serial, isolated, parse_growth_report,
};

const SUITE: &str = "crates/eval-core/tests/growth.rs::";

fn quota() -> ReviewerQuota {
    ReviewerQuota {
        receipt_charge_bytes: 32 << 10,
        job_allowance_bytes: 32 << 10,
        project_metadata_bytes: 64 << 20,
        host_metadata_bytes: 256 << 20,
    }
}

fn bounds() -> GrowthBounds {
    GrowthBounds {
        store_bytes: 64 << 20,
        artifact_objects: 4096,
        commit_log_rows: 100_000,
        projection_rows: 100_000,
        open_holds: 1,
    }
}

fn limits() -> ResourceLimits {
    ResourceLimits {
        elapsed_ms: 1_800_000,
        store_bytes: 512 << 20,
        cassette_bytes: 64 << 20,
        artifact_bytes: 256 << 20,
        temp_roots: 4,
        retained_artifacts: 32,
        processes: 6,
    }
}

fn headroom(pending: u64, terminal: u64) -> HeadroomSample {
    let q = quota();
    let bytes = q.receipt_charge_bytes * terminal
        + (q.receipt_charge_bytes + q.job_allowance_bytes) * pending;
    HeadroomSample {
        pending_jobs: pending,
        terminal_jobs: terminal,
        page_bytes: 0,
        project_metadata_bytes: bytes,
        project_metadata_remaining: q.project_metadata_bytes - bytes,
        admitted_total: pending + terminal,
        r24_refusals: 0,
    }
}

fn sample(step: u32, commit_seq: i64, wal: u64, tmp: u64) -> ResourceSample {
    ResourceSample {
        step,
        commit_seq,
        stores: StoreFamily::ALL
            .into_iter()
            .map(|f| {
                (
                    f,
                    StoreBytes {
                        file: 4096 * (step as u64 + 1),
                        wal,
                        shm: 0,
                    },
                )
            })
            .collect(),
        artifact_objects: step as u64,
        artifact_tmp_entries: tmp,
        artifact_bytes: 1024 * step as u64,
        cassette_bytes: 0,
        published_bytes: 0,
        temp_roots: 0,
        processes: 0,
        commit_log_rows: commit_seq as u64,
        projection_rows: step as u64 * 2,
        open_holds: 1,
        headroom: headroom(1, step as u64),
    }
}

fn ledger() -> GrowthLedger {
    let mut ledger = GrowthLedger::new(GrowthMode::NeverRestored);
    ledger.record(sample(1, 3, 4096, 0)).unwrap();
    ledger.record(sample(2, 7, 32768, 1)).unwrap();
    ledger.record(sample(3, 9, 0, 0)).unwrap();
    ledger
}

fn mix() -> SwarmMix {
    let mut mix = SwarmMix::default();
    for op in Operation::ALL {
        mix.record(op);
    }
    mix
}

fn report() -> GrowthReport {
    GrowthReport {
        schema: GROWTH_REPORT_SCHEMA.to_string(),
        eval_run_id: "ab".repeat(32),
        profile_digest: "cd".repeat(32),
        claim_boundary: ClaimBoundary::pinned(),
        quota: quota(),
        bounds: bounds(),
        ledger: ledger(),
        mix: mix(),
        expected_refusals: Vec::new(),
        fault_episodes: 2,
        safety_checks_while_armed: 2,
        markers: BTreeSet::new(),
        envelope: Envelope::new(limits()),
    }
}

#[test]
fn headroom_is_accounted_from_the_constants_read_not_a_slot_count() {
    let q = quota();
    assert_eq!(q.expected_project_bytes(&headroom(0, 0)), 0);
    assert_eq!(q.expected_project_bytes(&headroom(1, 0)), 64 << 10);
    assert_eq!(q.expected_project_bytes(&headroom(0, 1)), 32 << 10);
    assert_eq!(
        q.expected_project_bytes(&headroom(2, 3)),
        (2 * 64 + 3 * 32) << 10
    );
    let mut with_pages = headroom(1, 1);
    with_pages.page_bytes = 100;
    assert_eq!(q.expected_project_bytes(&with_pages), (96 << 10) + 100);
    assert_eq!(q.admissions_remaining(q.project_metadata_bytes), 1024);
    let mut other = q.clone();
    other.receipt_charge_bytes = 64 << 10;
    assert_eq!(
        other.admissions_remaining(other.project_metadata_bytes),
        682
    );
    let mut mismatched = ledger();
    mismatched.samples[1].headroom.project_metadata_bytes += 1;
    assert_eq!(
        mismatched.verdict(&q, &bounds()),
        Err(GrowthRefused::HeadroomMismatch {
            step: 2,
            expected: (64 + 2 * 32) << 10,
            observed: ((64 + 2 * 32) << 10) + 1,
        })
    );
}

#[test]
fn a_never_restored_ledger_passes_only_when_the_final_sample_holds_nothing_transient() {
    let ledger = ledger();
    ledger.verdict(&quota(), &bounds()).unwrap();
    assert_eq!(
        ledger.peak_store_bytes(),
        ledger.samples[1].store_total(),
        "the peak is the WAL-heavy middle sample, not the final size"
    );
    assert!(ledger.peak_store_bytes() > ledger.samples[2].store_total());
    let mut wal = ledger.clone();
    wal.samples[2]
        .stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .wal = 8192;
    assert_eq!(
        wal.verdict(&quota(), &bounds()),
        Err(GrowthRefused::Leak {
            resource: "wal_bytes_after_truncate".to_string(),
            step: 3,
            observed: 8192,
        })
    );
    let mut tmp = ledger.clone();
    tmp.samples[2].artifact_tmp_entries = 1;
    assert!(matches!(
        tmp.verdict(&quota(), &bounds()),
        Err(GrowthRefused::Leak { resource, step: 3, observed: 1 }) if resource == "artifact_tmp_entries"
    ));
    let mut roots = ledger.clone();
    roots.samples[2].temp_roots = 2;
    assert!(matches!(
        roots.verdict(&quota(), &bounds()),
        Err(GrowthRefused::Leak { resource, .. }) if resource == "temp_roots"
    ));
    let mut over = ledger.clone();
    over.samples[2].commit_log_rows = 200_000;
    assert_eq!(
        over.verdict(&quota(), &bounds()),
        Err(GrowthRefused::BoundExceeded {
            resource: "commit_log_rows".to_string(),
            step: 3,
            bound: 100_000,
            observed: 200_000,
        })
    );
    let mut empty = GrowthLedger::new(GrowthMode::NeverRestored);
    assert_eq!(
        empty.verdict(&quota(), &bounds()),
        Err(GrowthRefused::NoSamples)
    );
    empty.record(sample(4, 1, 0, 0)).unwrap();
    assert_eq!(
        empty.record(sample(4, 2, 0, 0)),
        Err(GrowthRefused::StepNotMonotonic { step: 4 })
    );
    assert_eq!(
        empty.record(sample(5, 0, 0, 0)),
        Err(GrowthRefused::CommitSeqNotMonotonic { step: 5 })
    );
}

#[test]
fn a_restore_under_never_restored_is_refused_and_a_restoring_ledger_gives_no_leak_verdict() {
    let mut coverage = Coverage::default();
    let mut never = ledger();
    assert_eq!(
        never.restore_attempted(),
        Err(GrowthRefused::RestoreUnderNeverRestored)
    );
    assert_eq!(never.restores_refused, 1);
    let mut restoring = GrowthLedger::new(GrowthMode::Restoring);
    restoring.restore_attempted().unwrap();
    restoring.record(sample(1, 1, 0, 0)).unwrap();
    assert_eq!(
        restoring.verdict(&quota(), &bounds()),
        Err(GrowthRefused::NotALeakVerdict {
            mode: GrowthMode::Restoring
        }),
        "a run that restored cannot say anything about leaks, whatever its samples show"
    );
    coverage
        .record("flt_restore_under_never_restored_refused")
        .unwrap();
}

#[test]
fn a_mix_missing_an_operation_is_not_sustainability_success() {
    let mut coverage = Coverage::default();
    mix().complete().unwrap();
    let mut partial = SwarmMix::default();
    for op in [Operation::Publish, Operation::Correct, Operation::Query] {
        partial.record(op);
        partial.record(op);
    }
    assert_eq!(
        partial.complete(),
        Err(MixIncomplete {
            missing: [
                Operation::Retire,
                Operation::FaultEpisode,
                Operation::QuotaPressure,
                Operation::StoreGrowth,
            ]
            .into_iter()
            .collect(),
        })
    );
    let mut report = report();
    report.mix = partial;
    assert!(matches!(report.validate(), Err(GrowthReportError::Mix(_))));
    coverage.record("flt_incomplete_mix_not_success").unwrap();
}

#[test]
fn a_shared_root_namespace_or_port_is_refused() {
    let mut coverage = Coverage::default();
    let a = CampaignResources {
        roots: ["/tmp/a".to_string()].into_iter().collect(),
        publish_dirs: ["/tmp/a/out".to_string()].into_iter().collect(),
        cassette_namespaces: ["world-a".to_string()].into_iter().collect(),
        ports: [40001].into_iter().collect(),
    };
    let b = CampaignResources {
        roots: ["/tmp/b".to_string()].into_iter().collect(),
        publish_dirs: ["/tmp/b/out".to_string()].into_iter().collect(),
        cassette_namespaces: ["world-b".to_string()].into_iter().collect(),
        ports: [40002].into_iter().collect(),
    };
    isolated(&a, &b).unwrap();
    let mut shared_root = b.clone();
    shared_root.roots.insert("/tmp/a".to_string());
    assert_eq!(
        isolated(&a, &shared_root),
        Err(IsolationRefused::SharedRoot {
            path: "/tmp/a".to_string()
        })
    );
    let mut shared_out = b.clone();
    shared_out.publish_dirs.insert("/tmp/a/out".to_string());
    assert!(matches!(
        isolated(&a, &shared_out),
        Err(IsolationRefused::SharedPublishDir { .. })
    ));
    let mut shared_ns = b.clone();
    shared_ns.cassette_namespaces.insert("world-a".to_string());
    assert!(matches!(
        isolated(&a, &shared_ns),
        Err(IsolationRefused::SharedCassetteNamespace { .. })
    ));
    let mut shared_port = b;
    shared_port.ports.insert(40001);
    assert_eq!(
        isolated(&a, &shared_port),
        Err(IsolationRefused::SharedPort { port: 40001 })
    );
    digests_match_serial(
        &["x".to_string(), "y".to_string()],
        &["x".to_string(), "y".to_string()],
    )
    .unwrap();
    assert_eq!(
        digests_match_serial(
            &["x".to_string(), "z".to_string()],
            &["x".to_string(), "y".to_string()]
        ),
        Err(IsolationRefused::DigestDiffersFromSerial { campaign: 1 })
    );
    coverage.record("xc_shared_fixture_refused").unwrap();
}

#[test]
fn a_growth_report_round_trips_and_its_digest_ignores_measurements() {
    let report = report();
    let value = report.serialize().unwrap();
    assert_eq!(parse_growth_report(&value).unwrap(), report);
    let digest = GrowthReport::result_digest(&value).unwrap();
    let mut other_machine = value.clone();
    other_machine["ledger"]["samples"][0]["stores"]["kernel"]["file"] = serde_json::json!(999_999);
    other_machine["envelope"]["peaks"]["store_bytes"] = serde_json::json!(999_999);
    assert_eq!(GrowthReport::result_digest(&other_machine).unwrap(), digest);
    let mut other_quota = value.clone();
    other_quota["quota"]["receipt_charge_bytes"] = serde_json::json!(1);
    assert_ne!(GrowthReport::result_digest(&other_quota).unwrap(), digest);
    let mut restoring = report.clone();
    restoring.ledger.mode = GrowthMode::Restoring;
    restoring.validate().unwrap();
    restoring.ledger.samples.clear();
    assert_eq!(
        restoring.validate(),
        Err(GrowthReportError::Growth(GrowthRefused::NoSamples))
    );
    let mut unsafe_run = report.clone();
    unsafe_run.safety_checks_while_armed = 0;
    assert_eq!(
        unsafe_run.validate(),
        Err(GrowthReportError::SafetyNeverChecked)
    );
    let mut leaked = report;
    leaked.ledger.samples[2].artifact_tmp_entries = 3;
    assert!(matches!(
        leaked.validate(),
        Err(GrowthReportError::Growth(GrowthRefused::Leak { .. }))
    ));
    let mut extra = value;
    extra["surprise"] = serde_json::json!(1);
    assert!(matches!(
        parse_growth_report(&extra),
        Err(GrowthReportError::Shape(_))
    ));
    let _ = BTreeMap::<String, u64>::new();
}

#[test]
fn growth_markers_each_name_a_test_here() {
    let mine: Vec<_> = eval_core::MARKERS
        .iter()
        .filter(|m| m.test.starts_with(SUITE))
        .collect();
    assert_eq!(mine.len(), 3);
    let mut coverage = Coverage::default();
    for marker in &mine {
        coverage.record(marker.name).unwrap();
    }
    coverage.complete(SUITE).unwrap();
}
