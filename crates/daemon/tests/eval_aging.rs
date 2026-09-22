#![cfg(all(unix, feature = "test-support"))]

mod support;

#[path = "../examples/eval_runner/aging.rs"]
#[allow(dead_code)]
mod aging;
#[path = "../examples/eval_runner/campaign.rs"]
#[allow(dead_code)]
mod campaign;

use std::collections::BTreeSet;
use std::path::PathBuf;

use aging::{
    Config, Full, MANIFEST_FILE, Plan, REPORT_FILE, Run, RunError, Stores, full_life, live, plan,
};
use campaign::Charges;
use eval_core::{
    AgingReport, Approval, CheckpointRefused, Construction, ConstructionKind, Coverage, Divergence,
    ExecutionMode, GuardComparison, MARKERS, PrefixRefused, ProfileError, RestoreRefused, Scale,
    StateSnapshot, StoreFamily, WindowDeaths, WorkCounter, parse_aging_report, parse_manifest,
};
use memory_store::MemoryStore;
use memory_store::memory_capture::CaptureSource;
use rusqlite::{Connection, OpenFlags};
use support::direct_host::example_binary;

const MESSAGES: u32 = 40;
const SUITE: &str = "crates/daemon/tests/eval_aging.rs::";

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

fn a_quiescent_copy_resumes_the_full_replay_in_one_incarnation_scenario(coverage: &mut Coverage) {
    let publish = tempfile::tempdir().unwrap();
    let out = publish.path().join("out");
    let run: Run = aging::run(&config(out.clone(), budget().unwrap_or(600_000))).unwrap();
    for marker in run.coverage.fired() {
        coverage.record(marker).unwrap();
    }
    let report = &run.report;
    assert_ne!(run.checkpoint.incarnation_id, run.full_incarnation_id);
    assert!(report.commit_seq_at_checkpoint < report.commit_seq_at_end);
    assert_eq!(report.full_guard_digest, report.resumed_guard_digest);
    assert!(report.against_resumed.live_digests_equal);
    assert!(report.against_bulk.live_digests_equal);
    assert_eq!(report.against_resumed.later.kind, ConstructionKind::Bulk);
    assert_eq!(
        report.against_resumed.later.snapshot_commit_seq,
        report.commit_seq_at_checkpoint
    );
    assert_eq!(
        report.against_resumed.earlier.kind,
        ConstructionKind::CatchUp
    );
    assert!(report.against_resumed.earlier.snapshot_commit_seq < report.commit_seq_at_checkpoint);
    for comparison in [&report.against_resumed, &report.against_bulk] {
        assert!(
            comparison
                .divergences
                .iter()
                .any(|d| matches!(d, Divergence::TombstonedBeforeSnapshot { .. })),
            "{comparison:?}"
        );
    }
    assert!(report.against_bulk.divergences.len() > report.against_resumed.divergences.len());
    assert!(report.window_deaths.supersessions > 0 && report.window_deaths.retirements > 0);
    for (family, store) in &run.checkpoint.receipt.stores {
        assert_eq!(
            store.pending.keys().copied().collect::<Vec<_>>(),
            family.counters(),
            "{family:?}"
        );
        assert!(store.pending.values().all(|n| *n == 0), "{store:?}");
        assert!(store.wal.is_truncated(), "{store:?}");
        assert!(store.wal.wal_frames >= 0, "{store:?}");
        assert_eq!(store.wal_sidecar_bytes, 0, "{store:?}");
        assert!(store.handles_closed, "{store:?}");
    }
    assert_eq!(run.checkpoint.receipt.stores.len(), 3);
    assert_eq!(run.checkpoint.receipt.step, report.checkpoint_step);
    let copied: BTreeSet<&str> = run.checkpoint.files.keys().map(String::as_str).collect();
    for file in [
        "kernel/kernel.sqlite",
        "memory.sqlite",
        "search/search.sqlite",
    ] {
        assert!(copied.contains(file), "{copied:?}");
    }
    assert!(
        copied
            .iter()
            .any(|f| f.starts_with("kernel/artifacts/objects/")),
        "{copied:?}"
    );

    let mut slipped = run.resumed.clone();
    slipped.memory.pop_last();
    assert_eq!(
        StateSnapshot::compare(&run.full, &slipped),
        Err(PrefixRefused::HistorySlipped {
            family: StoreFamily::Memory,
        })
    );
    coverage.record("flt_prefix_history_slipped").unwrap();

    let published: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join(REPORT_FILE)).unwrap()).unwrap();
    assert_eq!(parse_aging_report(&published).unwrap(), *report);
    let manifest = parse_manifest(
        &serde_json::from_slice(&std::fs::read(out.join(MANIFEST_FILE)).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.execution_mode, ExecutionMode::PrefixThenGenerate);
    assert_eq!(manifest.construction, Construction::Replay);
    assert_eq!(manifest.eval_run_id, report.eval_run_id);
    assert_eq!(
        manifest.result_digest,
        AgingReport::result_digest(&published).unwrap()
    );
    assert_eq!(manifest.run_identity.root_seed, aging::SEED);
}

fn a_copy_with_pending_work_is_refused_by_the_counter_it_left_scenario(coverage: &mut Coverage) {
    let plan = plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..3]);
    stores.apply(&plan.steps[3]);
    let closed = stores.close();
    assert_eq!(closed.receipt.step, 4);
    assert!(
        closed.receipt.stores[&StoreFamily::Kernel].pending[&WorkCounter::OutboxUnpublished] > 0,
        "{:?}",
        closed.receipt
    );
    coverage.record("flt_copy_attempted_mid_episode").unwrap();
    let into = tempfile::tempdir().unwrap();
    assert!(matches!(
        closed.copy(into.path()).err().unwrap(),
        CheckpointRefused::PendingWork {
            family: StoreFamily::Kernel,
            counter: WorkCounter::OutboxUnpublished,
            ..
        }
    ));
    assert!(
        std::fs::read_dir(into.path()).unwrap().next().is_none(),
        "a refused copy writes nothing"
    );
}

fn a_reader_holding_the_projection_leaves_the_checkpoint_busy_scenario(coverage: &mut Coverage) {
    let plan = plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..3]);
    let reader =
        Connection::open_with_flags(stores.projection_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _held: i64 = reader
        .query_row("SELECT COUNT(*) FROM occurrences", [], |row| row.get(0))
        .unwrap();
    let closed = stores.close();
    let wal = closed.receipt.stores[&StoreFamily::SearchProjection].wal;
    assert_ne!(wal.busy, 0, "{wal:?}");
    coverage.record("flt_checkpoint_observed_busy").unwrap();
    let into = tempfile::tempdir().unwrap();
    assert!(matches!(
        closed.copy(into.path()).err().unwrap(),
        CheckpointRefused::WalNotTruncated {
            family: StoreFamily::SearchProjection,
            ..
        }
    ));
    reader.execute_batch("COMMIT").unwrap();
}

fn a_copy_beside_a_live_memory_store_handle_is_refused_scenario(coverage: &mut Coverage) {
    let plan = plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..3]);
    let closed = stores.close();
    let live_handle = MemoryStore::open(&daemon::store_descriptor_in(closed.root())).unwrap();
    coverage
        .record("sls_memstore_copy_refused_live_handle")
        .unwrap();
    let into = tempfile::tempdir().unwrap();
    assert_eq!(
        closed.copy(into.path()).err().unwrap(),
        CheckpointRefused::HandleOpen {
            family: StoreFamily::Memory,
        }
    );
    drop(live_handle);
}

fn a_foreign_incarnation_is_refused_at_reopen_scenario(coverage: &mut Coverage) {
    let plan = plan(MESSAGES).unwrap();
    let k = plan.checkpoint_step as usize;
    let one = tempfile::tempdir().unwrap();
    let mut first = Stores::open(one.path(), plan.rendering.clone());
    live(&mut first, &plan.steps[..k]);
    let other = tempfile::tempdir().unwrap();
    let mut second = Stores::open(other.path(), plan.rendering.clone());
    live(&mut second, &plan.steps[..k]);
    assert_ne!(first.incarnation(), second.incarnation());
    coverage
        .record("flt_foreign_incarnation_refused_at_reopen")
        .unwrap();
    let first_copy = tempfile::tempdir().unwrap();
    let (checkpoint, _) = first.close().copy(first_copy.path()).unwrap();
    let into = tempfile::tempdir().unwrap();
    let (_, copied) = second.close().copy(into.path()).unwrap();
    // The persisted incarnation id lives in the kernel file, so a foreign
    // copy's bytes differ there before the identity check reads them.
    assert_eq!(
        copied
            .reopen(&checkpoint, plan.steps[k].now_ms)
            .err()
            .unwrap(),
        RestoreRefused::FileDiffers {
            path: "kernel/kernel.sqlite".to_string(),
        }
    );
    let (own, copied, _kept) = plan_copy(&plan, k);
    let object = own
        .files
        .keys()
        .find(|f| f.starts_with("kernel/artifacts/objects/"))
        .unwrap()
        .clone();
    std::fs::remove_file(copied.root().join(&object)).unwrap();
    assert_eq!(
        copied.reopen(&own, plan.steps[k].now_ms).err().unwrap(),
        RestoreRefused::FileMissing { path: object }
    );
}

fn plan_copy(
    plan: &aging::Plan,
    k: usize,
) -> (eval_core::Checkpoint, aging::Copied, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..k]);
    let into = tempfile::tempdir().unwrap();
    let (checkpoint, copied) = stores.close().copy(into.path()).unwrap();
    (checkpoint, copied, into)
}

#[test]
fn a_copy_beside_a_live_kernel_handle_is_refused() {
    let plan = plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..3]);
    let closed = stores.close();
    assert!(closed.receipt.stores[&StoreFamily::Kernel].handles_closed);
    let live_handle = kernel::KernelStore::open(closed.root().join("kernel")).unwrap();
    let into = tempfile::tempdir().unwrap();
    assert_eq!(
        closed.copy(into.path()).err().unwrap(),
        CheckpointRefused::HandleOpen {
            family: StoreFamily::Kernel,
        }
    );
    assert!(
        std::fs::read_dir(into.path()).unwrap().next().is_none(),
        "a refused copy writes nothing"
    );
    drop(live_handle);
}

#[test]
fn work_enqueued_between_the_close_and_the_copy_is_refused() {
    let plan = plan(MESSAGES).unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..3]);
    let closed = stores.close();
    assert_eq!(
        closed.receipt.stores[&StoreFamily::Memory].pending[&WorkCounter::CaptureJobsPending],
        0
    );
    // Another holder takes the released lease, leaves work, and lets go
    // before the copy's probe runs.
    let holder = MemoryStore::open(&daemon::store_descriptor_in(closed.root())).unwrap();
    holder
        .enqueue_memory_capture(
            CaptureSource {
                project: "project-0",
                harness: "pi",
                session_id: "session-0",
                message_id: "message-0",
                role: "user",
                text: "left behind",
            },
            0,
        )
        .unwrap();
    drop(holder);
    let into = tempfile::tempdir().unwrap();
    assert!(matches!(
        closed.copy(into.path()).err().unwrap(),
        CheckpointRefused::PendingWork {
            family: StoreFamily::Memory,
            counter: WorkCounter::CaptureJobsPending,
            observed: 1,
        }
    ));
    assert!(
        std::fs::read_dir(into.path()).unwrap().next().is_none(),
        "a refused copy writes nothing"
    );
}

#[test]
fn a_copy_missing_a_store_file_is_refused_at_reopen() {
    let plan = plan(MESSAGES).unwrap();
    for file in [
        "kernel/kernel.sqlite",
        "memory.sqlite",
        "search/search.sqlite",
    ] {
        let (checkpoint, copied, _kept) = plan_copy(&plan, 3);
        assert!(checkpoint.files.contains_key(file), "{file}");
        std::fs::remove_file(copied.root().join(file)).unwrap();
        assert_eq!(
            copied
                .reopen(&checkpoint, plan.steps[3].now_ms)
                .err()
                .unwrap(),
            RestoreRefused::FileMissing {
                path: file.to_string(),
            }
        );
    }
}

#[test]
fn a_copy_with_a_modified_store_file_is_refused_at_reopen() {
    let plan = plan(MESSAGES).unwrap();
    for file in [
        "kernel/kernel.sqlite",
        "memory.sqlite",
        "search/search.sqlite",
    ] {
        let (checkpoint, copied, _kept) = plan_copy(&plan, 3);
        std::fs::write(copied.root().join(file), b"not a database").unwrap();
        assert_eq!(
            copied
                .reopen(&checkpoint, plan.steps[3].now_ms)
                .err()
                .unwrap(),
            RestoreRefused::FileDiffers {
                path: file.to_string(),
            }
        );
    }
}

#[test]
fn a_wal_sidecar_whose_metadata_cannot_be_read_is_not_recorded_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("store.sqlite");
    std::fs::write(&file, b"").unwrap();
    let wal = dir.path().join("store.sqlite-wal");
    // A self-referential symlink makes `metadata` fail with ELOOP, an error
    // that is not `NotFound`.
    std::os::unix::fs::symlink(&wal, &wal).unwrap();
    assert!(std::fs::metadata(&wal).is_err());
    assert!(
        std::panic::catch_unwind(|| aging::sidecar_len(&file)).is_err(),
        "an unreadable sidecar must not be recorded as empty"
    );
}

#[test]
fn an_unapproved_profile_refuses_before_any_store_opens() {
    let publish = tempfile::tempdir().unwrap();
    let mut config = config(publish.path().join("out"), 600_000);
    config.approval = None;
    // A history this long is never generated: the refusal comes first.
    config.messages = u32::MAX;
    assert!(matches!(
        aging::run(&config).err().unwrap(),
        RunError::Profile(ProfileError::NotApproved { .. })
    ));
    assert!(!publish.path().join("out").exists());
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn the_example_publishes_the_same_digests_as_the_in_process_run() {
    let budget_ms = budget_or_panic();
    let publish = tempfile::tempdir().unwrap();
    let run = aging::run(&config(publish.path().join("in-process"), budget_ms)).unwrap();
    let out = publish.path().join("cli");
    let binary = example_binary("eval_runner", "eval-runner");
    let output = std::process::Command::new(&binary)
        .args([
            "aging",
            "--scale",
            "s0",
            "--messages",
            &MESSAGES.to_string(),
            "--elapsed-bound-ms",
            &budget_ms.to_string(),
            "--approved-by",
            "maintainer",
            "--approval-run-id",
            &"ab".repeat(32),
            "--publish",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["status"], "completed");
    let published: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join(REPORT_FILE)).unwrap()).unwrap();
    let cli = parse_aging_report(&published).unwrap();
    assert_ne!(cli.checkpoint_digest, run.report.checkpoint_digest);
    assert_eq!(cli.full_guard_digest, run.report.full_guard_digest);
    assert_eq!(cli.resumed_guard_digest, run.report.resumed_guard_digest);
    assert_eq!(cli.against_resumed, run.report.against_resumed);
    assert_eq!(cli.against_bulk, run.report.against_bulk);
    assert_eq!(cli.window_deaths, run.report.window_deaths);
    let manifest = parse_manifest(
        &serde_json::from_slice(&std::fs::read(out.join(MANIFEST_FILE)).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.result_digest,
        AgingReport::result_digest(&published).unwrap()
    );
    let refused = std::process::Command::new(&binary)
        .args(["aging", "--scale", "s0"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--messages is required"));
}

type Scenario = fn(&mut Coverage);

fn scenarios() -> [(&'static str, Scenario); 5] {
    [
        (
            "a_quiescent_copy_resumes_the_full_replay_in_one_incarnation",
            a_quiescent_copy_resumes_the_full_replay_in_one_incarnation_scenario,
        ),
        (
            "a_copy_with_pending_work_is_refused_by_the_counter_it_left",
            a_copy_with_pending_work_is_refused_by_the_counter_it_left_scenario,
        ),
        (
            "a_reader_holding_the_projection_leaves_the_checkpoint_busy",
            a_reader_holding_the_projection_leaves_the_checkpoint_busy_scenario,
        ),
        (
            "a_copy_beside_a_live_memory_store_handle_is_refused",
            a_copy_beside_a_live_memory_store_handle_is_refused_scenario,
        ),
        (
            "a_foreign_incarnation_is_refused_at_reopen",
            a_foreign_incarnation_is_refused_at_reopen_scenario,
        ),
    ]
}

fn run(name: &str) {
    let (_, scenario) = scenarios().into_iter().find(|(n, _)| *n == name).unwrap();
    let mut coverage = Coverage::default();
    scenario(&mut coverage);
    let owned: BTreeSet<&str> = MARKERS
        .iter()
        .filter(|m| m.test == format!("{SUITE}{name}"))
        .map(|m| m.name)
        .collect();
    assert!(!owned.is_empty(), "{name} owns a marker");
    for marker in owned {
        assert!(coverage.fired().contains(marker), "{name} records {marker}");
    }
}

#[test]
fn a_quiescent_copy_resumes_the_full_replay_in_one_incarnation() {
    run("a_quiescent_copy_resumes_the_full_replay_in_one_incarnation");
}

#[test]
fn a_copy_with_pending_work_is_refused_by_the_counter_it_left() {
    run("a_copy_with_pending_work_is_refused_by_the_counter_it_left");
}

#[test]
fn a_reader_holding_the_projection_leaves_the_checkpoint_busy() {
    run("a_reader_holding_the_projection_leaves_the_checkpoint_busy");
}

#[test]
fn a_copy_beside_a_live_memory_store_handle_is_refused() {
    run("a_copy_beside_a_live_memory_store_handle_is_refused");
}

#[test]
fn a_foreign_incarnation_is_refused_at_reopen() {
    run("a_foreign_incarnation_is_refused_at_reopen");
}

#[test]
fn aging_markers_each_name_a_scenario_here() {
    let scenario_names: BTreeSet<&str> = scenarios().iter().map(|(n, _)| *n).collect();
    for marker in MARKERS.iter().filter(|m| m.test.starts_with(SUITE)) {
        let test = marker.test.strip_prefix(SUITE).unwrap();
        assert!(scenario_names.contains(test), "{test}");
    }
}

#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn every_aging_marker_fires_across_the_scenarios() {
    budget_or_panic();
    let mut coverage = Coverage::default();
    for (_, scenario) in scenarios() {
        scenario(&mut coverage);
    }
    coverage.complete(SUITE).unwrap();
}

fn charges() -> Charges {
    let profile = campaign::profile(Scale::S0, 128, 600_000, None);
    Charges::new(profile.envelope)
}

fn straddling_plan() -> Plan {
    let plan = plan(MESSAGES).unwrap();
    assert!(plan.checkpoint_step > 0);
    assert!((plan.checkpoint_step as usize) < plan.steps.len());
    plan
}

#[test]
fn the_aged_arm_is_built_by_replay_and_matches_the_bulk_scaffold_only_by_enumerated_deaths() {
    let plan = straddling_plan();
    let mut charges = charges();
    let full: Full = full_life(&plan, &mut charges).unwrap();
    assert_eq!(full.incarnation_id.len(), 32);
    assert!(full.state.commit_seq > 0);
    assert_eq!(full.against_bulk.earlier.kind, ConstructionKind::CatchUp);
    assert_eq!(full.against_bulk.later.kind, ConstructionKind::Bulk);
    assert!(
        full.against_bulk.earlier.snapshot_commit_seq < full.against_bulk.later.snapshot_commit_seq
    );
    assert!(full.against_bulk.live_digests_equal);
    assert!(
        full.against_bulk
            .divergences
            .iter()
            .any(|d| matches!(d, Divergence::TombstonedBeforeSnapshot { .. }))
    );
    let root = tempfile::tempdir().unwrap();
    let mut prefix = Stores::open(root.path(), plan.rendering.clone());
    live(&mut prefix, &plan.steps[..plan.checkpoint_step as usize]);
    let checkpoint_tip = prefix.tip();
    assert!(checkpoint_tip < full.state.commit_seq);
    let window = WindowDeaths::count(&full.state.kernel, checkpoint_tip, full.state.commit_seq);
    assert!(window.supersessions > 0);
    assert!(window.retirements > 0);
    assert!(full.state.kernel.values().any(|d| {
        d.invalidated_commit_seq
            .is_some_and(|at| at <= checkpoint_tip)
    }));
}

#[test]
fn two_lives_of_one_history_share_a_guard_digest_and_a_slipped_family_is_named() {
    let plan = straddling_plan();
    let mut charges = charges();
    let first = full_life(&plan, &mut charges).unwrap();
    let second = full_life(&plan, &mut charges).unwrap();
    assert_ne!(first.incarnation_id, second.incarnation_id);
    StateSnapshot::compare(&first.state, &second.state).unwrap();
    assert_eq!(
        first.state.guard_digest().unwrap(),
        second.state.guard_digest().unwrap()
    );
    let both = GuardComparison::of(
        (&first.rows, ConstructionKind::CatchUp),
        (&second.rows, ConstructionKind::CatchUp),
    )
    .unwrap();
    assert!(both.live_digests_equal);
    assert!(both.divergences.is_empty());

    let root = tempfile::tempdir().unwrap();
    let mut short = Stores::open(root.path(), plan.rendering.clone());
    live(&mut short, &plan.steps[..plan.checkpoint_step as usize]);
    let prefix = short.snapshot();
    assert_eq!(
        StateSnapshot::compare(&first.state, &prefix),
        Err(PrefixRefused::CommitSeqDiffers {
            full: first.state.commit_seq,
            resumed: prefix.commit_seq,
        })
    );
    let mut slipped = first.state.clone();
    slipped.memory.clear();
    assert_eq!(
        StateSnapshot::compare(&first.state, &slipped),
        Err(PrefixRefused::HistorySlipped {
            family: StoreFamily::Memory,
        })
    );
}

#[test]
fn a_history_too_short_to_straddle_a_death_is_refused() {
    assert!(matches!(plan(2), Err(RunError::NoStraddlingStep)));
}
