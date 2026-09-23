use std::collections::BTreeSet;

use eval_core::{
    CampaignResources, ClaimBoundary, Coverage, Envelope, EnvelopeExceeded, ExpectedRefusal,
    GROWTH_REPORT_SCHEMA, GrowthBounds, GrowthContract, GrowthLedger, GrowthMode, GrowthRefused,
    GrowthReport, GrowthReportError, HeadroomSample, IsolationRefused, MixIncomplete, Operation,
    RecordedRefusal, Resource, ResourceLimits, ResourceSample, ReviewerQuota, StoreBytes,
    StoreFamily, SwarmMix, digests_match_serial, isolated, parse_growth_report,
};

const SUITE: &str = "crates/eval-core/tests/growth.rs::";

fn quota() -> ReviewerQuota {
    ReviewerQuota {
        receipt_charge_bytes: 32 << 10,
        job_allowance_bytes: 32 << 10,
        page_receipt_bytes: 4096,
        page_allowance_bytes: 64 << 10,
        project_metadata_bytes: 64 << 20,
        host_metadata_bytes: 256 << 20,
    }
}

fn bounds() -> GrowthBounds {
    GrowthBounds {
        store_bytes: 64 << 20,
        artifact_objects: 4096,
        artifact_bytes: 16 << 20,
        commit_log_rows: 100_000,
        projection_rows: 100_000,
        open_holds: 1,
        store_bytes_per_commit: 64 << 10,
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

fn contract() -> GrowthContract {
    GrowthContract {
        quota: quota(),
        bounds: bounds(),
        envelope: limits(),
    }
}

fn headroom(pending: u64, terminal: u64) -> HeadroomSample {
    let q = quota();
    let bytes = q.receipt_charge_bytes * terminal
        + (q.receipt_charge_bytes + q.job_allowance_bytes) * pending;
    HeadroomSample {
        pending_jobs: pending,
        terminal_jobs: terminal,
        frozen_pages: 0,
        terminal_pages: 0,
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
    let ledger = ledger();
    let mut envelope = Envelope::new(limits());
    envelope.peaks.store_bytes = ledger.peak_store_bytes();
    GrowthReport {
        schema: GROWTH_REPORT_SCHEMA.to_string(),
        eval_run_id: "ab".repeat(32),
        profile_digest: "cd".repeat(32),
        claim_boundary: ClaimBoundary::pinned(),
        quota: quota(),
        bounds: bounds(),
        ledger,
        mix: mix(),
        expected_refusals: Vec::new(),
        fault_episodes: 1,
        safety_checks_while_armed: 2,
        markers: BTreeSet::new(),
        envelope,
    }
}

#[test]
fn headroom_is_accounted_from_the_constants_read_not_a_slot_count() {
    let q = quota();
    assert_eq!(q.expected_project_bytes(&headroom(0, 0)), Some(0));
    assert_eq!(q.expected_project_bytes(&headroom(1, 0)), Some(64 << 10));
    assert_eq!(q.expected_project_bytes(&headroom(0, 1)), Some(32 << 10));
    assert_eq!(
        q.expected_project_bytes(&headroom(2, 3)),
        Some((2 * 64 + 3 * 32) << 10)
    );
    let mut with_pages = headroom(1, 1);
    with_pages.frozen_pages = 2;
    with_pages.terminal_pages = 3;
    assert_eq!(
        q.expected_project_bytes(&with_pages),
        Some((96 << 10) + 2 * (4096 + (64 << 10)) + 3 * 4096),
        "a frozen page holds its receipt and allowance, a terminal page its receipt"
    );
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
    let mut remaining = ledger();
    remaining.samples[1].headroom.project_metadata_remaining += 1;
    let expected = q.project_metadata_bytes - ((64 + 2 * 32) << 10);
    assert_eq!(
        remaining.verdict(&q, &bounds()),
        Err(GrowthRefused::RemainingMismatch {
            step: 2,
            expected,
            observed: expected + 1,
        }),
        "the remaining bytes must follow from the quota and the bytes charged"
    );
    let mut over_quota = ledger();
    over_quota.samples[2].headroom.pending_jobs = 2048;
    over_quota.samples[2].headroom.admitted_total = 2048 + 3;
    let held = q
        .expected_project_bytes(&over_quota.samples[2].headroom)
        .unwrap();
    over_quota.samples[2].headroom.project_metadata_bytes = held;
    over_quota.samples[2].headroom.project_metadata_remaining = 0;
    assert_eq!(
        over_quota.verdict(&q, &bounds()),
        Err(GrowthRefused::HeadroomOverQuota {
            step: 3,
            expected: held,
            quota: 64 << 20,
        }),
        "held bytes above the quota contradict admission, and are not an exhausted quota"
    );
    let mut wide_charge = q.clone();
    wide_charge.receipt_charge_bytes = u64::MAX;
    wide_charge.job_allowance_bytes = 1;
    assert_eq!(
        wide_charge.admissions_remaining(1),
        0,
        "a charge past u64 admits nothing, and does not panic"
    );
    let mut huge = ledger();
    huge.samples[2].headroom.terminal_jobs = u64::MAX;
    huge.samples[2].headroom.admitted_total = u64::MAX;
    assert_eq!(
        huge.verdict(&q, &bounds()),
        Err(GrowthRefused::HeadroomOverflow { step: 3 }),
        "job counts the constants cannot multiply are refused, not wrapped"
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
    over.samples[2].projection_rows = 200_000;
    assert_eq!(
        over.verdict(&quota(), &bounds()),
        Err(GrowthRefused::BoundExceeded {
            resource: "projection_rows".to_string(),
            step: 3,
            bound: 100_000,
            observed: 200_000,
        })
    );
    let mut rate = ledger.clone();
    rate.samples[2]
        .stores
        .get_mut(&StoreFamily::Kernel)
        .unwrap()
        .file = 64 << 20;
    assert!(
        matches!(
            rate.verdict(
                &quota(),
                &GrowthBounds {
                    store_bytes: 128 << 20,
                    ..bounds()
                }
            ),
            Err(GrowthRefused::GrowthRateExceeded { commits: 6, .. })
        ),
        "growth faster than the per-commit allowance is a leak even under the size bound"
    );
    let mut wal_baseline = ledger.clone();
    wal_baseline.samples[0]
        .stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .wal = 8 << 20;
    wal_baseline.samples[2]
        .stores
        .get_mut(&StoreFamily::Kernel)
        .unwrap()
        .file = 8 << 20;
    assert!(
        matches!(
            wal_baseline.verdict(&quota(), &bounds()),
            Err(GrowthRefused::GrowthRateExceeded { commits: 6, .. })
        ),
        "a WAL-heavy first sample must not cancel the file bytes the history retained"
    );
    let mut idle = GrowthLedger::new(GrowthMode::NeverRestored);
    idle.record(sample(1, 3, 0, 0)).unwrap();
    idle.record(sample(2, 3, 0, 0)).unwrap();
    assert_eq!(
        idle.verdict(&quota(), &bounds()),
        Err(GrowthRefused::GrowthRateExceeded {
            bytes_per_commit: 64 << 10,
            observed_bytes: 3 * 4096,
            commits: 0,
        }),
        "file bytes added with no commit between the samples have no allowance"
    );
    let mut hidden = ledger.clone();
    hidden.samples[2]
        .stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .wal = 8192;
    hidden.samples[2].stores.remove(&StoreFamily::Memory);
    assert_eq!(
        hidden.verdict(&quota(), &bounds()),
        Err(GrowthRefused::StoreMissing {
            step: 3,
            family: StoreFamily::Memory,
        }),
        "a sample that omits a store family cannot hide that store's bytes"
    );
    let mut single = GrowthLedger::new(GrowthMode::NeverRestored);
    single.record(sample(3, 9, 0, 0)).unwrap();
    assert_eq!(
        single.verdict(&quota(), &bounds()),
        Err(GrowthRefused::NoBaseline),
        "one sample has no interval to judge growth over"
    );
    let mut negative = ledger.clone();
    negative.samples[0].commit_seq = -100;
    assert_eq!(
        negative.verdict(&quota(), &bounds()),
        Err(GrowthRefused::CommitSeqNegative { step: 1 }),
        "a negative baseline would buy allowance for commits that never happened"
    );
    let mut fat = ledger.clone();
    fat.samples[2]
        .stores
        .get_mut(&StoreFamily::Kernel)
        .unwrap()
        .file = u64::MAX;
    assert!(
        matches!(
            fat.verdict(&quota(), &bounds()),
            Err(GrowthRefused::GrowthRateExceeded { .. })
        ),
        "store bytes past u64 saturate into a refusal, not a panic"
    );
    let mut receding_r24 = ledger.clone();
    receding_r24.samples[1].headroom.r24_refusals = 2;
    assert_eq!(
        receding_r24.verdict(&quota(), &bounds()),
        Err(GrowthRefused::CountNotMonotonic {
            step: 3,
            field: "r24_refusals",
        })
    );
    let mut forgotten = ledger.clone();
    forgotten.samples[2].headroom.terminal_jobs = 1;
    forgotten.samples[2].headroom.admitted_total = 2;
    forgotten.samples[2].headroom.project_metadata_bytes = 96 << 10;
    forgotten.samples[2].headroom.project_metadata_remaining = (64 << 20) - (96 << 10);
    assert_eq!(
        forgotten.verdict(&quota(), &bounds()),
        Err(GrowthRefused::CountNotMonotonic {
            step: 3,
            field: "terminal_jobs",
        }),
        "a terminal job is a permanent receipt; forgetting one restores headroom that was spent"
    );
    let mut unadmitted = ledger.clone();
    unadmitted.samples[1].headroom.admitted_total += 1;
    assert_eq!(
        unadmitted.verdict(&quota(), &bounds()),
        Err(GrowthRefused::AdmittedMismatch {
            step: 2,
            expected: 3,
            observed: 4,
        })
    );
    let mut pruned = GrowthLedger::new(GrowthMode::NeverRestored);
    pruned.record(sample(1, 3, 0, 0)).unwrap();
    let mut idle_final = sample(2, 3, 0, 0);
    idle_final.stores = pruned.samples[0].stores.clone();
    idle_final.commit_log_rows = 2;
    assert_eq!(
        pruned.record(idle_final.clone()),
        Err(GrowthRefused::CountNotMonotonic {
            step: 2,
            field: "commit_log_rows",
        })
    );
    pruned.samples.push(idle_final);
    assert_eq!(
        pruned.verdict(&quota(), &bounds()),
        Err(GrowthRefused::CountNotMonotonic {
            step: 2,
            field: "commit_log_rows",
        }),
        "the commit log is append-only; fewer rows than before is not retention"
    );
    let mut thawed = ledger.clone();
    thawed.samples[1].headroom.frozen_pages = 1;
    thawed.samples[1].headroom.project_metadata_bytes += 4096 + (64 << 10);
    thawed.samples[1].headroom.project_metadata_remaining -= 4096 + (64 << 10);
    assert_eq!(
        thawed.verdict(&quota(), &bounds()),
        Err(GrowthRefused::CountNotMonotonic {
            step: 3,
            field: "pages",
        }),
        "a page that stops being frozen becomes terminal; it does not vanish"
    );
    let mut rowless = ledger.clone();
    rowless.samples[2].commit_seq = 12;
    assert_eq!(
        rowless.verdict(&quota(), &bounds()),
        Err(GrowthRefused::CommitRowsDisagree {
            commits: 9,
            rows: 6,
        }),
        "a sequence advance the commit log did not retain buys no allowance"
    );
    let mut reordered = ledger.clone();
    reordered.samples.swap(1, 2);
    assert_eq!(
        reordered.verdict(&quota(), &bounds()),
        Err(GrowthRefused::StepNotMonotonic { step: 2 }),
        "a ledger built without `record` must still be in step order"
    );
    let mut receding = ledger.clone();
    receding.samples[2].commit_seq = 5;
    assert_eq!(
        receding.verdict(&quota(), &bounds()),
        Err(GrowthRefused::CommitSeqNotMonotonic { step: 3 })
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
    assert!(matches!(
        report.validate(&contract()),
        Err(GrowthReportError::Mix(_))
    ));
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
    assert_eq!(
        isolated(&a, &shared_out),
        Err(IsolationRefused::SharedRoot {
            path: "/tmp/a".to_string()
        }),
        "a's publish directory lies under a's root, so sharing it shares the root"
    );
    let mut outside = a.clone();
    outside.publish_dirs = ["/srv/out-a".to_string()].into_iter().collect();
    let mut shared_outside = b.clone();
    shared_outside.publish_dirs.insert("/srv/out-a".to_string());
    assert_eq!(
        isolated(&outside, &shared_outside),
        Err(IsolationRefused::SharedPublishDir {
            path: "/srv/out-a".to_string()
        })
    );
    let mut publishes_into_root = b.clone();
    publishes_into_root
        .publish_dirs
        .insert("/tmp/a".to_string());
    assert_eq!(
        isolated(&a, &publishes_into_root),
        Err(IsolationRefused::SharedRoot {
            path: "/tmp/a".to_string()
        }),
        "one campaign's publish directory must not be another's root"
    );
    let mut rooted_in_out = b.clone();
    rooted_in_out.roots.insert("/srv/out-a".to_string());
    assert_eq!(
        isolated(&outside, &rooted_in_out),
        Err(IsolationRefused::SharedPublishDir {
            path: "/srv/out-a".to_string()
        }),
        "one campaign's root must not be another's publish directory"
    );
    let mut slashed = a.clone();
    slashed.roots = ["/tmp/a/".to_string()].into_iter().collect();
    let mut nested_under_slashed = b.clone();
    nested_under_slashed
        .roots
        .insert("/tmp/a/nested".to_string());
    assert_eq!(
        isolated(&slashed, &nested_under_slashed),
        Err(IsolationRefused::SharedRoot {
            path: "/tmp/a/".to_string()
        }),
        "a trailing separator does not hide a nested root"
    );
    let mut fs_root = b.clone();
    fs_root.roots.insert("/".to_string());
    assert_eq!(
        isolated(&a, &fs_root),
        Err(IsolationRefused::SharedRoot {
            path: "/tmp/a".to_string()
        }),
        "the filesystem root contains every campaign"
    );
    let mut nested = b.clone();
    nested.roots.insert("/tmp/a/nested".to_string());
    assert_eq!(
        isolated(&a, &nested),
        Err(IsolationRefused::SharedRoot {
            path: "/tmp/a".to_string()
        }),
        "a root inside another campaign's root writes into it"
    );
    let mut parent = b.clone();
    parent.publish_dirs.insert("/tmp".to_string());
    assert_eq!(
        isolated(&a, &parent),
        Err(IsolationRefused::SharedRoot {
            path: "/tmp/a".to_string()
        }),
        "a publish directory above another campaign's root contains it"
    );
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
        digests_match_serial(&["x".to_string()], &["x".to_string(), "y".to_string()]),
        Err(IsolationRefused::DigestDiffersFromSerial { campaign: 1 }),
        "a concurrent run that produced fewer digests is not a match"
    );
    assert_eq!(
        digests_match_serial(
            &["x".to_string(), "z".to_string()],
            &["x".to_string(), "y".to_string()]
        ),
        Err(IsolationRefused::DigestDiffersFromSerial { campaign: 1 })
    );
    assert_eq!(
        digests_match_serial(&[], &[]),
        Err(IsolationRefused::TooFewCampaigns { campaigns: 0 }),
        "no campaigns is no isolation evidence"
    );
    assert_eq!(
        digests_match_serial(&["x".to_string()], &["x".to_string()]),
        Err(IsolationRefused::TooFewCampaigns { campaigns: 1 })
    );
    coverage.record("xc_shared_fixture_refused").unwrap();
}

#[test]
fn a_growth_report_round_trips_and_its_digest_ignores_measurements() {
    let report = report();
    let value = report.serialize(&contract()).unwrap();
    assert_eq!(parse_growth_report(&value, &contract()).unwrap(), report);
    let digest = GrowthReport::result_digest(&value).unwrap();
    let mut other_machine = value.clone();
    other_machine["ledger"]["samples"][0]["stores"]["kernel"]["file"] = serde_json::json!(999_999);
    other_machine["ledger"]["samples"][1]["stores"]["memory"]["wal"] = serde_json::json!(1);
    other_machine["ledger"]["samples"][2]["artifact_bytes"] = serde_json::json!(999_999);
    other_machine["ledger"]["samples"][2]["cassette_bytes"] = serde_json::json!(999_999);
    other_machine["envelope"]["peaks"]["store_bytes"] = serde_json::json!(999_999);
    assert_eq!(GrowthReport::result_digest(&other_machine).unwrap(), digest);
    let mut other_history = value.clone();
    other_history["ledger"]["samples"][2]["commit_seq"] = serde_json::json!(10);
    other_history["ledger"]["samples"][2]["commit_log_rows"] = serde_json::json!(10);
    assert_ne!(
        GrowthReport::result_digest(&other_history).unwrap(),
        digest,
        "a campaign that saw another campaign's commits must not match its serial digest"
    );
    let mut other_headroom = value.clone();
    other_headroom["ledger"]["samples"][0]["headroom"]["admitted_total"] = serde_json::json!(7);
    assert_ne!(
        GrowthReport::result_digest(&other_headroom).unwrap(),
        digest
    );
    let mut other_quota = value.clone();
    other_quota["quota"]["receipt_charge_bytes"] = serde_json::json!(1);
    assert_ne!(GrowthReport::result_digest(&other_quota).unwrap(), digest);
    let mut restoring = report.clone();
    restoring.ledger.mode = GrowthMode::Restoring;
    restoring.validate(&contract()).unwrap();
    let mut refused_while_restoring = restoring.clone();
    refused_while_restoring.ledger.restores_refused = 1;
    assert_eq!(
        refused_while_restoring.validate(&contract()),
        Err(GrowthReportError::Growth(
            GrowthRefused::RestoresRefusedUnderRestoring {
                restores_refused: 1
            }
        )),
        "a restoring ledger permits every restore, so it refused none"
    );
    let mut restoring_headroom = restoring.clone();
    restoring_headroom.ledger.samples[1]
        .headroom
        .project_metadata_bytes += 1;
    assert_eq!(
        restoring_headroom.validate(&contract()),
        Err(GrowthReportError::Growth(GrowthRefused::HeadroomMismatch {
            step: 2,
            expected: (64 + 2 * 32) << 10,
            observed: ((64 + 2 * 32) << 10) + 1,
        })),
        "restoring suppresses the leak verdict, not the headroom evidence"
    );
    restoring.ledger.samples.clear();
    assert_eq!(
        restoring.validate(&contract()),
        Err(GrowthReportError::Growth(GrowthRefused::NoSamples))
    );
    let mut unsafe_run = report.clone();
    unsafe_run.safety_checks_while_armed = 0;
    assert_eq!(
        unsafe_run.validate(&contract()),
        Err(GrowthReportError::SafetyNeverChecked)
    );
    let mut uncounted = report.clone();
    uncounted.fault_episodes = 0;
    uncounted.safety_checks_while_armed = 0;
    assert_eq!(
        uncounted.validate(&contract()),
        Err(GrowthReportError::SafetyNeverChecked),
        "the mix exercised a fault episode, so a zero episode count cannot waive the safety check"
    );
    let mut faultless = report.clone();
    faultless.fault_episodes = 0;
    assert_eq!(
        faultless.validate(&contract()),
        Err(GrowthReportError::FaultEpisodesDisagree {
            declared: 0,
            exercised: 1,
        }),
        "the mix exercised a fault episode the count denies"
    );
    let mut paged = report.clone();
    paged.ledger.samples[2].headroom.frozen_pages = 1;
    assert!(
        matches!(
            paged.validate(&contract()),
            Err(GrowthReportError::Growth(GrowthRefused::HeadroomMismatch {
                step: 3,
                ..
            }))
        ),
        "a frozen page is charged from the constants, not a byte figure the report supplies"
    );
    let mut restoring_reordered = report.clone();
    restoring_reordered.ledger.mode = GrowthMode::Restoring;
    restoring_reordered.ledger.samples[2].commit_seq = 5;
    assert_eq!(
        restoring_reordered.validate(&contract()),
        Err(GrowthReportError::Growth(
            GrowthRefused::CommitSeqNotMonotonic { step: 3 }
        ))
    );
    let mut reordered = report.clone();
    reordered.ledger.samples.swap(0, 2);
    assert_eq!(
        parse_growth_report(&serde_json::to_value(&reordered).unwrap(), &contract()),
        Err(GrowthReportError::Growth(GrowthRefused::StepNotMonotonic {
            step: 2
        })),
        "a report read back is held to the order `record` enforces"
    );
    let mut leaked = report.clone();
    leaked.ledger.samples[2].artifact_tmp_entries = 3;
    assert!(matches!(
        leaked.validate(&contract()),
        Err(GrowthReportError::Growth(GrowthRefused::Leak { .. }))
    ));
    let mut breached = report.clone();
    breached.envelope.peaks.store_bytes = limits().store_bytes + 1;
    assert_eq!(
        breached.validate(&contract()),
        Err(GrowthReportError::EnvelopeNotHonoured(EnvelopeExceeded {
            resource: Resource::StoreBytes,
            bound: limits().store_bytes,
            observed: limits().store_bytes + 1,
        })),
        "a report whose envelope peaks crossed a bound is the run the envelope stops"
    );
    let mut store_artifacts = report.clone();
    store_artifacts.ledger.samples[2].artifact_bytes = 999_999;
    store_artifacts
        .validate(&contract())
        .expect("artifact-store bytes are not the published bytes the envelope charges");
    let mut generous = report.clone();
    generous.quota.project_metadata_bytes = 128 << 20;
    for sample in &mut generous.ledger.samples {
        sample.headroom.project_metadata_remaining += 64 << 20;
    }
    assert_eq!(
        generous.validate(&contract()),
        Err(GrowthReportError::QuotaMismatch),
        "quota constants the producer inflated are not the store's"
    );
    let mut raised = report.clone();
    raised.envelope.bounds.store_bytes = u64::MAX;
    raised.envelope.peaks.store_bytes = limits().store_bytes + 1;
    assert_eq!(
        raised.validate(&contract()),
        Err(GrowthReportError::EnvelopeBoundsNotApproved),
        "envelope bounds the producer widened are not the approved limits"
    );
    let mut misnamed = report.clone();
    misnamed.eval_run_id = "not-a-digest".to_string();
    assert_eq!(
        misnamed.validate(&contract()),
        Err(GrowthReportError::MalformedDigest {
            field: "eval_run_id"
        })
    );
    let mut unprofiled = report.clone();
    unprofiled.profile_digest.truncate(10);
    assert_eq!(
        unprofiled.validate(&contract()),
        Err(GrowthReportError::MalformedDigest {
            field: "profile_digest"
        })
    );
    let mut unpinned = report.clone();
    unpinned.claim_boundary.exclusions.clear();
    assert_eq!(
        unpinned.validate(&contract()),
        Err(GrowthReportError::ClaimBoundaryMismatch)
    );
    let mut uncharged = report.clone();
    let peak = report.ledger.peak_store_bytes();
    uncharged.envelope.peaks.store_bytes = peak - 1;
    assert_eq!(
        uncharged.validate(&contract()),
        Err(GrowthReportError::EnvelopeNotCharged {
            resource: Resource::StoreBytes,
            step: 2,
            peak: peak - 1,
            observed: peak,
        }),
        "a sample the envelope peak never saw was not charged to it"
    );
    let mut loosened = report.clone();
    loosened.ledger.samples[2].projection_rows = 200_000;
    loosened.bounds.projection_rows = 300_000;
    assert_eq!(
        loosened.validate(&contract()),
        Err(GrowthReportError::BoundsNotApproved),
        "bounds the producer widened are not the approved bounds"
    );
    loosened.bounds = bounds();
    assert!(
        matches!(
            loosened.validate(&contract()),
            Err(GrowthReportError::Growth(
                GrowthRefused::BoundExceeded { .. }
            ))
        ),
        "held to the approved bounds, the widened sample is over"
    );
    let mut r24_counted = report.clone();
    r24_counted.ledger.samples[2].headroom.r24_refusals = 1;
    assert_eq!(
        r24_counted.validate(&contract()),
        Err(GrowthReportError::R24Unreconciled {
            counted: 1,
            recorded: 0,
        })
    );
    let mut r24_recorded = report.clone();
    r24_recorded.expected_refusals.push(RecordedRefusal {
        episode: "quota".to_string(),
        refusal: ExpectedRefusal::R24ReceiptQuotaExhausted,
        production_error: "refused: MetadataQuota".to_string(),
    });
    assert_eq!(
        r24_recorded.validate(&contract()),
        Err(GrowthReportError::R24Unreconciled {
            counted: 0,
            recorded: 1,
        })
    );
    r24_recorded.ledger.samples[2].headroom.r24_refusals = 1;
    r24_recorded.expected_refusals.push(RecordedRefusal {
        episode: "catch-up".to_string(),
        refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
        production_error: "Blocked::DeletionUnpropagated".to_string(),
    });
    r24_recorded
        .validate(&contract())
        .expect("one R24 counted and one recorded agree; an R11 is not counted");
    let mut relabeled = report.clone();
    relabeled.ledger.samples[2].headroom.r24_refusals = 1;
    relabeled.expected_refusals.push(RecordedRefusal {
        episode: "quota".to_string(),
        refusal: ExpectedRefusal::R24ReceiptQuotaExhausted,
        production_error: "HostCapacity".to_string(),
    });
    assert_eq!(
        relabeled.validate(&contract()),
        Err(GrowthReportError::RefusalNotEvidenced {
            episode: "quota".to_string(),
            refusal: ExpectedRefusal::R24ReceiptQuotaExhausted,
        }),
        "a refusal is expected only when production named it"
    );
    let mut invented = report.clone();
    invented
        .markers
        .insert("flt_marker_nobody_owns".to_string());
    assert_eq!(
        invented.validate(&contract()),
        Err(GrowthReportError::UnregisteredMarker {
            marker: "flt_marker_nobody_owns".to_string(),
        })
    );
    let mut unsafe_int = report;
    unsafe_int.ledger.restores_refused = 1 << 53;
    assert!(
        matches!(
            unsafe_int.serialize(&contract()),
            Err(GrowthReportError::NotCanonical(_))
        ),
        "an integer past the safe range would be accepted and then refused by the digest"
    );
    let mut wide_value = value.clone();
    wide_value["ledger"]["restores_refused"] = serde_json::json!(1u64 << 53);
    assert!(matches!(
        parse_growth_report(&wide_value, &contract()),
        Err(GrowthReportError::NotCanonical(_))
    ));
    let mut extra = value;
    extra["surprise"] = serde_json::json!(1);
    assert!(matches!(
        parse_growth_report(&extra, &contract()),
        Err(GrowthReportError::Shape(_))
    ));
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
