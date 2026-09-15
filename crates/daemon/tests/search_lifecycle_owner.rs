//! Exercises the daemon's lifecycle owner from a recorded request through selection, completion, pinned reads, disable, authorized recovery, and restart, then the same convergence through a running daemon.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daemon::projection_admission::{ADMISSION_DIR, EVIDENCE_RECORD, InputRefusal};
use daemon::projection_gates::{Denial, EntryPoint, ProjectionHook};
use daemon::projection_lifecycle::{
    Cause, ConsumerBinding, ControlState, LifecycleRequest, ProjectionLifecycle, Transition,
};
use daemon::search_catchup::{Blocked, EpisodeEnd};
use daemon::search_lifecycle_owner::{
    IDENTITY_CONTRACT_VERSION, PROJECTION_POLICY_VERSION, SearchLifecycleOwner, SliceOutcome,
    SpecRefusal,
};
use daemon::search_replacement::BuildError;
use daemon::search_replacement::selection::disable::DisableEvent;
use daemon::search_replacement::selection::recovery::RecoveryProgress;
use host_runtime::local_embeddings::{LocalEmbeddingsComponent, LocalEmbeddingsLimits};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{KernelStore, ProjectScope};
use serde_json::Value;
use support::embedding_fixtures::{
    Corpus, GENERATION, TestEngine, budget, component, identity, kernel_incarnation_id,
};
use support::kernel_daemon::KernelDaemon;
use support::projection_gate::{
    LAG_LIMIT, campaign_json, manifest_json, manifest_json_with, write_records,
};

/// The consumer the rebuilt family registers; the corpus fixture already registers `search`.
const CONSUMER: &str = "search-lifecycle";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn slice_budget() -> EvalBudget {
    budget(Duration::from_secs(20))
}

fn lane() -> LocalEmbeddingsComponent {
    component(&TestEngine::new(), LocalEmbeddingsLimits::default())
}

fn owner(home: &Path, kernel: &Arc<KernelStore>) -> SearchLifecycleOwner {
    SearchLifecycleOwner::for_home(home, Arc::clone(kernel), lane())
}

/// A rebuild of an unregistered home under this test's consumer and the fixture generation.
fn rebuild(home: &Path) -> LifecycleRequest {
    LifecycleRequest {
        transition: Transition::Rebuilding,
        selected_generation: "unregistered".to_owned(),
        kernel_incarnation_id: kernel_incarnation_id(home),
        consumer: ConsumerBinding {
            consumer_id: CONSUMER.to_owned(),
            generation_id: GENERATION.to_owned(),
        },
        cause: Cause::DeletedAfterPruning,
        attempt_id: "rebuild-attempt".to_owned(),
        recovery_target: None,
        allowance: 3,
        deadline: now() + 60_000,
        authorization_ref: None,
    }
}

fn records(home: &Path) {
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json(&identity, &ProjectionHook::ALL),
        &campaign_json(&identity),
    );
}

fn control(home: &Path) -> ControlState {
    ProjectionLifecycle::open(home).unwrap().read()
}

/// The identity constants agree with the frozen construction contract and the source policy the fixtures were built under.
#[test]
fn identity_constants_match_the_construction_contract() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../kernel/tests/fixtures/search-projection/construction-contracts.json");
    let contracts: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        contracts["identity_contract_version"],
        IDENTITY_CONTRACT_VERSION
    );
    assert_eq!(
        PROJECTION_POLICY_VERSION,
        support::embedding_fixtures::POLICY
    );
}

/// AC1, AC2: without records or a ready lane the owner closes admission and refuses requests; with both, a recorded rebuild reaches Current only across successive slices, the selected family serves pinned reads whose coverage the gate then judges, and withholding the second slice leaves the record short of Current.
#[test]
fn a_recorded_rebuild_reaches_current_across_scheduled_slices() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let object = corpus.publish("kept", "kept text");
    let owner = owner(home, &corpus.kernel);

    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Closed(SpecRefusal::Inputs(InputRefusal::Missing(_)))
    ));
    assert!(matches!(
        owner.request(&rebuild(home), now(), &slice_budget()),
        Err(BuildError::Intent(_))
    ));
    assert_eq!(control(home), ControlState::Absent);

    records(home);
    let disabled_lane = SearchLifecycleOwner::for_home(
        home,
        Arc::clone(&corpus.kernel),
        LocalEmbeddingsComponent::unsupported("no lane"),
    );
    assert!(matches!(
        disabled_lane.run_slice(&slice_budget()),
        SliceOutcome::Closed(SpecRefusal::LaneNotReady("disabled"))
    ));

    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Unregistered
    ));
    assert!(matches!(
        owner.pin(&slice_budget()),
        Err(BuildError::Invalid(_))
    ));
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap()
        .expect("a rebuild is recorded");
    assert!(matches!(control(home), ControlState::Intent(_)));

    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(outcome, SliceOutcome::Advanced(RecoveryProgress::Selected)),
        "{outcome:?}"
    );
    assert!(
        matches!(control(home), ControlState::Intent(_)),
        "one slice selects; it does not publish Current"
    );
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Advanced(RecoveryProgress::Current)
    ));
    assert!(matches!(control(home), ControlState::Current(_)));
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));

    let reader = owner.pin(&slice_budget()).unwrap();
    let coverage = reader.coverage(&slice_budget()).unwrap();
    assert_eq!(coverage.class(OccurrenceClass::Messages).lexical, 1);
    assert_eq!(coverage.checkpoint.checkpoint_commit_seq, corpus.tip());
    let rows: Vec<String> = reader
        .read(&slice_budget(), |conn| {
            Ok(conn
                .prepare("SELECT source_object_id FROM occurrences")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(rows, vec![object]);
    let grant = owner
        .admission()
        .gate()
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .unwrap();
    assert!(!grant.invalidated.is_cancelled());

    std::fs::remove_file(home.join(ADMISSION_DIR).join(EVIDENCE_RECORD)).unwrap();
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Closed(SpecRefusal::Inputs(InputRefusal::Missing(EVIDENCE_RECORD)))
    ));
    assert!(
        grant.invalidated.is_cancelled(),
        "withdrawn records cancel grants"
    );
    assert!(matches!(
        owner.pin(&slice_budget()),
        Err(BuildError::Invalid(_))
    ));
}

/// AC2, AC4: a restarted owner resumes the Current record without a new request and without renewing its allowance; a kernel of another incarnation is refused rather than adopted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_resumes_current_and_refuses_another_kernel() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(home);
    let before = {
        let owner = owner(home, &corpus.kernel);
        assert!(matches!(
            owner.run_slice(&slice_budget()),
            SliceOutcome::Unregistered
        ));
        owner
            .request(&rebuild(home), now(), &slice_budget())
            .unwrap();
        for _ in 0..2 {
            assert!(matches!(
                owner.run_slice(&slice_budget()),
                SliceOutcome::Advanced(_)
            ));
        }
        owner.shutdown().await.unwrap();
        match control(home) {
            ControlState::Current(intent) => intent,
            state => panic!("not Current: {state:?}"),
        }
    };

    let owner = owner(home, &corpus.kernel);
    assert_eq!(
        owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Startup)
            .unwrap_err(),
        Denial::NoManifest,
        "a restart starts closed"
    );
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    let after = match control(home) {
        ControlState::Current(intent) => intent,
        state => panic!("not Current: {state:?}"),
    };
    assert_eq!(after.episodes, before.episodes, "a restart renews nothing");
    assert_eq!(after.attempt_id, before.attempt_id);
    owner.pin(&slice_budget()).unwrap();

    let other = tempfile::tempdir().unwrap();
    let other_corpus = Corpus::open(other.path());
    other_corpus.seed();
    let foreign = SearchLifecycleOwner::for_home(home, Arc::clone(&other_corpus.kernel), lane());
    assert!(matches!(
        foreign.run_slice(&slice_budget()),
        SliceOutcome::Blocked(reason) if reason.contains("identity")
    ));
    assert!(matches!(
        foreign.pin(&slice_budget()),
        Err(BuildError::Invalid(_) | BuildError::Mutation(_) | BuildError::Denied(_))
    ));
}

/// AC2, AC6: disable closes admission before anything is written, reconciles the consumer, leaves the record Disabled across a restart, and only an authorized recovery request reopens it; that recovery then reaches Current across slices.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disable_persists_across_restart_and_authorized_recovery_reaches_current() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let corpus = Corpus::open(&home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(&home);
    let owner = Arc::new(owner(&home, &corpus.kernel));
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(&home), now(), &slice_budget())
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            owner.run_slice(&slice_budget()),
            SliceOutcome::Advanced(_)
        ));
    }
    let staged = match control(&home) {
        ControlState::Current(intent) => intent.staged_seed_digest.unwrap(),
        state => panic!("not Current: {state:?}"),
    };
    let mut events = Vec::new();
    owner
        .disable(&slice_budget(), &mut |event| events.push(event))
        .await
        .unwrap();
    assert!(matches!(
        events.first(),
        Some(DisableEvent::AdmissionClosed)
    ));
    assert!(matches!(control(&home), ControlState::Disabled(_)));
    assert_eq!(
        owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::RecoveryRequired
    );
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Disabled
    ));
    assert!(owner.pin(&slice_budget()).is_err());

    let restarted = SearchLifecycleOwner::for_home(&home, Arc::clone(&corpus.kernel), lane());
    assert!(matches!(
        restarted.run_slice(&slice_budget()),
        SliceOutcome::Disabled
    ));
    assert!(restarted.pin(&slice_budget()).is_err());

    let mut recovery = rebuild(&home);
    recovery.transition = Transition::AuthorizedRecovery;
    recovery.cause = Cause::DisabledRecovery;
    recovery.selected_generation = staged;
    recovery.attempt_id = "recovery-attempt".to_owned();
    recovery.consumer.consumer_id = "search-recovered".to_owned();
    recovery.consumer.generation_id = "gen-2".to_owned();
    recovery.authorization_ref = Some("operator:recovery-ticket".to_owned());
    restarted
        .request(&recovery, now(), &slice_budget())
        .unwrap();
    assert!(matches!(control(&home), ControlState::Intent(_)));
    for _ in 0..2 {
        assert!(matches!(
            restarted.run_slice(&slice_budget()),
            SliceOutcome::Advanced(_)
        ));
    }
    assert!(matches!(control(&home), ControlState::Current(_)));
    let reader = restarted.pin(&slice_budget()).unwrap();
    assert_eq!(reader.consumer().consumer_id, "search-recovered");
}

/// AC1: a recorded rebuild converges to Current through the running daemon's scheduled slices alone, and the daemon's pinned access serves the family until shutdown closes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn the_running_daemon_converges_a_recorded_rebuild_and_pins_it() {
    let data = tempfile::tempdir().unwrap();
    let home = data.path().to_owned();
    let home = home.as_path();
    let kernel_root = home.join("eidnara").join("context");
    // The kernel the daemon opens lives under the managed directory; a store opened here first seeds it so the request can name its incarnation.
    {
        let seed = KernelStore::open(kernel_root.join("kernel")).unwrap();
        drop(seed);
    }
    let incarnation = kernel_incarnation_id(&kernel_root);
    let identity = identity(&incarnation);
    write_records(
        home,
        &manifest_json(&identity, &ProjectionHook::ALL),
        &campaign_json(&identity),
    );
    let mut request = rebuild(&kernel_root);
    request.kernel_incarnation_id = incarnation;
    ProjectionLifecycle::open(home)
        .unwrap()
        .record(&support::projection_gate::open_gate(), &request, now())
        .unwrap();

    let daemon = KernelDaemon::start_in(data, None).await;
    let started = Instant::now();
    let owner = loop {
        if let Some(owner) = daemon.handler().search_lifecycle() {
            break owner;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    // The daemon's slice holds the lifecycle writer lock while it runs, so a probe that lands inside a slice retries.
    let mut observed = None;
    loop {
        if let Ok(lifecycle) = ProjectionLifecycle::open(home) {
            observed = Some(lifecycle.read());
            if matches!(observed, Some(ControlState::Current(_))) {
                break;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "scheduled slices did not reach Current: {observed:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let reader = tokio::task::spawn_blocking({
        let owner = Arc::clone(&owner);
        move || owner.pin(&slice_budget())
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reader.consumer().consumer_id, CONSUMER);
    drop(reader);
    daemon.shutdown().await;
    assert_eq!(
        owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::NoManifest,
        "shutdown closes admission"
    );
    assert!(owner.pin(&slice_budget()).is_err());
}

/// AC1, AC7: an active record bounds every slice by its own deadline, so a caller's unbounded budget still yields the finite budget selection requires, a record about to expire starts nothing and consumes no episode, and a restart between selection and completion resumes the same record without renewing its allowance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slices_are_bounded_by_the_records_deadline_and_a_restart_renews_nothing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());

    let mut expiring = rebuild(home);
    expiring.deadline = now() + 500;
    owner.request(&expiring, now(), &slice_budget()).unwrap();
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Blocked(_)
    ));
    let ControlState::Intent(intent) = control(home) else {
        panic!("the expiring record stays recorded");
    };
    assert_eq!(
        intent.episodes.consumed, 0,
        "no episode was spent inside the margin"
    );
    std::thread::sleep(Duration::from_millis(600));
    std::fs::remove_dir_all(home.join("search-lifecycle")).unwrap();

    let mut short = rebuild(home);
    short.deadline = now() + 8_000;
    owner.request(&short, now(), &slice_budget()).unwrap();
    assert!(matches!(
        owner.run_slice(&EvalBudget::unbounded()),
        SliceOutcome::Advanced(RecoveryProgress::Selected)
    ));
    let ControlState::Intent(selected) = control(home) else {
        panic!("one slice selects only");
    };
    owner.shutdown().await.unwrap();

    let resumed = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane());
    assert!(matches!(
        resumed.run_slice(&EvalBudget::unbounded()),
        SliceOutcome::Advanced(RecoveryProgress::Current)
    ));
    let ControlState::Current(done) = control(home) else {
        panic!("the resumed record reached Current");
    };
    assert_eq!(done.attempt_id, selected.attempt_id);
    assert_eq!(done.episodes.allowance, selected.episodes.allowance);
    assert_eq!(done.episodes.deadline, selected.episodes.deadline);
    assert!(done.episodes.consumed > selected.episodes.consumed);
    assert!(done.episodes.consumed <= done.episodes.allowance);
}

/// A Current family within the freshness limit catches up in the next slice under the hold its checkpoint names and is at the tip afterwards; after the kernel lease changes, the hold is dead, so the same slice reports the blocked episode while the family still serves; and a family that trails the kernel past the freshness limit is denied on its own coverage, so nothing runs on stale rows until a rebuild is requested.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_current_family_catches_up_under_its_hold_until_the_lease_changes() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    for _ in 0..2 {
        let _ = owner.run_slice(&slice_budget());
    }
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));

    let later = corpus.publish("later", "later text");
    let outcome = owner.run_slice(&slice_budget());
    let SliceOutcome::CaughtUp(report) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(report.end, EpisodeEnd::ReachedTarget);
    assert!(report.batches_applied >= 1);
    assert_eq!(report.acknowledged_through, corpus.tip());
    let reader = owner.pin(&slice_budget()).unwrap();
    assert_eq!(
        reader
            .coverage(&slice_budget())
            .unwrap()
            .checkpoint
            .checkpoint_commit_seq,
        corpus.tip()
    );
    let rows: Vec<String> = reader
        .read(&slice_budget(), |conn| {
            Ok(conn
                .prepare("SELECT source_object_id FROM occurrences ORDER BY created_commit_seq")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(rows.last(), Some(&later));
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        Some(corpus.tip()),
        "the kernel acknowledged only what the projection applied"
    );
    drop(reader);
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    owner.shutdown().await.unwrap();
    drop(owner);

    drop(corpus);
    let corpus = Corpus::open(home);
    let restarted = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane());
    assert!(matches!(
        restarted.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    corpus.publish("after-restart", "text");
    let outcome = restarted.run_slice(&slice_budget());
    let SliceOutcome::CaughtUp(report) = outcome else {
        panic!("{outcome:?}");
    };
    assert!(
        matches!(report.end, EpisodeEnd::Blocked(Blocked::HoldExtension(_))),
        "{report:?}"
    );
    assert_eq!(report.batches_applied, 0);
    restarted
        .pin(&slice_budget())
        .expect("a family within the freshness limit still serves");
}

/// A Current family that trails the kernel past the freshness limit is judged on its own coverage and denied before catch-up can run, so the slice reports the block rather than a fabricated observation and a rebuild is the way back.
#[test]
fn a_current_family_that_trails_the_kernel_is_denied_on_its_own_coverage() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    for _ in 0..2 {
        let _ = owner.run_slice(&slice_budget());
    }
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));

    let checkpoint = owner
        .pin(&slice_budget())
        .unwrap()
        .coverage(&slice_budget())
        .unwrap()
        .checkpoint
        .checkpoint_commit_seq;
    for index in 0..=LAG_LIMIT {
        corpus.publish(&format!("later-{index}"), "later text");
    }
    let lag = corpus.tip() - checkpoint;
    assert!(lag > i64::try_from(LAG_LIMIT).unwrap());
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::Blocked(reason) if reason.contains(&format!("trails the kernel by {lag}"))),
        "{outcome:?}"
    );
    assert_eq!(
        owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::Stale {
            lag,
            max: LAG_LIMIT
        }
    );
}

/// AC2: a corrupt control record is unavailable, closes admission, and pins nothing; an owner that has run no slice refuses a disable and an authorized recovery; and while a disable reconciles, a concurrent slice and pin see the family as disabled rather than rebuilding a second manager over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn corrupt_records_and_reconciling_disables_admit_nothing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let corpus = Corpus::open(&home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(&home);
    let fresh = owner(&home, &corpus.kernel);
    assert!(matches!(
        fresh.disable(&slice_budget(), &mut |_| {}).await,
        Err(BuildError::Invalid(_))
    ));
    let mut recovery = rebuild(&home);
    recovery.transition = Transition::AuthorizedRecovery;
    recovery.cause = Cause::DisabledRecovery;
    recovery.authorization_ref = Some("operator:ticket".to_owned());
    assert!(matches!(
        fresh.request(&recovery, now(), &slice_budget()),
        Err(BuildError::Invalid(_))
    ));

    let owner = Arc::new(owner(&home, &corpus.kernel));
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(&home), now(), &slice_budget())
        .unwrap();
    for _ in 0..2 {
        let _ = owner.run_slice(&slice_budget());
    }
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));

    let (entered, exited) = (std::sync::mpsc::channel(), std::sync::mpsc::channel());
    let (entered_tx, entered_rx) = entered;
    let (exit_tx, exit_rx) = exited;
    let probe = {
        let owner = Arc::clone(&owner);
        tokio::task::spawn_blocking(move || {
            entered_rx.recv().unwrap();
            let slice = owner.run_slice(&slice_budget());
            let pin = owner.pin(&slice_budget()).err();
            exit_tx.send(()).unwrap();
            (slice, pin)
        })
    };
    let mut notified = false;
    owner
        .disable(&slice_budget(), &mut |event| {
            if matches!(event, DisableEvent::IntentPersisted) && !notified {
                notified = true;
                entered_tx.send(()).unwrap();
                exit_rx.recv().unwrap();
            }
        })
        .await
        .unwrap();
    let (slice, pin) = probe.await.unwrap();
    assert!(matches!(slice, SliceOutcome::Disabled), "{slice:?}");
    assert!(matches!(pin, Some(BuildError::Invalid(_))), "{pin:?}");
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Disabled
    ));

    std::fs::write(home.join("search-lifecycle").join("intent.json"), b"{").unwrap();
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Unavailable(_)
    ));
    assert!(matches!(
        owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::NoManifest | Denial::RecoveryRequired
    ));
    assert!(owner.pin(&slice_budget()).is_err());
}

/// A manifest whose limits cannot bound one slice refuses the specification and closes admission, naming the limit, instead of recording work every slice would then deny.
#[test]
fn limits_too_small_to_bound_a_slice_refuse_the_specification() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let identity = identity(&kernel_incarnation_id(home));
    let owner = owner(home, &corpus.kernel);
    for (overrides, expected) in [
        (
            &[("local_transaction_rows", 9)][..],
            SpecRefusal::TooSmall("local_transaction_rows"),
        ),
        (
            &[("export_page_rows", 0)][..],
            SpecRefusal::TooSmall("export_page_rows"),
        ),
    ] {
        write_records(
            home,
            &manifest_json_with(&identity, &ProjectionHook::ALL, overrides),
            &campaign_json(&identity),
        );
        let outcome = owner.run_slice(&slice_budget());
        assert!(
            matches!(&outcome, SliceOutcome::Closed(refusal) if *refusal == expected),
            "{overrides:?}: {outcome:?}"
        );
    }
    records(home);
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    for (overrides, expected) in [
        (
            &[("retry_attempts", 2)][..],
            SpecRefusal::LimitRange("retry_attempts"),
        ),
        (
            &[("local_transaction_bytes", 100_000)][..],
            SpecRefusal::LimitRange("local_transaction_bytes"),
        ),
        (
            &[("capture_disk_bytes", 10 * 65_536)][..],
            SpecRefusal::LimitRange("capture_disk_bytes"),
        ),
        (
            &[("B_recovery_ms", 5)][..],
            SpecRefusal::LimitRange("B_recovery_ms"),
        ),
    ] {
        write_records(
            home,
            &manifest_json_with(&identity, &ProjectionHook::ALL, overrides),
            &campaign_json(&identity),
        );
        let outcome = owner.run_slice(&slice_budget());
        assert!(
            matches!(&outcome, SliceOutcome::Closed(refusal) if *refusal == expected),
            "{overrides:?}: {outcome:?}"
        );
        assert!(matches!(control(home), ControlState::Intent(_)));
        assert!(owner.pin(&slice_budget()).is_err());
    }
}

/// Vectors published in the selected family, keyed by source object.
fn published(owner: &SearchLifecycleOwner) -> Vec<String> {
    owner
        .pin(&slice_budget())
        .unwrap()
        .read(&slice_budget(), |conn| {
            Ok(conn
                .prepare(
                    "SELECT o.source_object_id FROM occurrence_vectors v JOIN occurrences o USING(occurrence_id) ORDER BY o.source_object_id",
                )?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap()
}

/// Runs slices until `done` holds or `limit` slices ran, joining every supervisor a slice hands back as the daemon's loop does.
async fn drive(
    owner: &SearchLifecycleOwner,
    limit: usize,
    mut done: impl FnMut() -> bool,
) -> usize {
    for slice in 1..=limit {
        if done() {
            return slice - 1;
        }
        match owner.run_slice(&slice_budget()) {
            SliceOutcome::RotateMaintenance(handle) => {
                owner.stop_maintenance(&handle).await.unwrap();
            }
            SliceOutcome::Blocked(reason) => panic!("blocked: {reason}"),
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    limit
}

/// AC1, S1, S2: with two bound projects each holding pending rows, one supervisor at a time rotates between them by tenure, and both projects' rows are embedded within a bounded number of slices; a project that leaves the roster loses its supervisor at the next slice, a lone project keeps its supervisor past the tenure, and no roster starts nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn maintenance_rotates_one_supervisor_across_bound_projects() {
    use support::embedding_fixtures::{PROJECT, PROJECT_B, SCOPE_B};
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let a = corpus.publish("a-row", "alpha text");
    let b = corpus.publish_scoped("b-row", "beta text", SCOPE_B);
    records(home);
    let engine = TestEngine::new();
    let roster = Arc::new(std::sync::Mutex::new(vec![
        ("project:a".to_owned(), ProjectScope::new(PROJECT).unwrap()),
        (
            "project:b".to_owned(),
            ProjectScope::new(PROJECT_B).unwrap(),
        ),
    ]));
    let owner = {
        let roster = Arc::clone(&roster);
        SearchLifecycleOwner::for_home(
            home,
            Arc::clone(&corpus.kernel),
            component(&engine, LocalEmbeddingsLimits::default()),
        )
        .with_roster(Arc::new(move || roster.lock().unwrap().clone()))
    };
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            owner.run_slice(&slice_budget()),
            SliceOutcome::Advanced(_)
        ));
    }
    assert!(
        owner.maintenance().is_none(),
        "nothing starts before Current is observed"
    );

    let slices = drive(&owner, 40, || published(&owner).len() == 2).await;
    assert!(slices <= 40, "both projects embedded within the bound");
    assert_eq!(published(&owner), {
        let mut both = vec![a.clone(), b.clone()];
        both.sort();
        both
    });
    assert!(engine.calls() >= 2);

    roster.lock().unwrap().truncate(1);
    let live = owner.maintenance();
    drive(&owner, 12, || {
        owner
            .maintenance()
            .is_none_or(|current| *current.scope == ProjectScope::new(PROJECT).unwrap())
    })
    .await;
    let last = owner.run_slice(&slice_budget());
    let lone = owner
        .maintenance()
        .unwrap_or_else(|| panic!("the lone project keeps a supervisor: {last:?}"));
    assert_eq!(*lone.scope, ProjectScope::new(PROJECT).unwrap());
    assert!(
        live.is_none_or(|earlier| {
            *earlier.scope != ProjectScope::new(PROJECT_B).unwrap() || {
                owner
                    .maintenance()
                    .is_some_and(|c| c.scope != earlier.scope)
            }
        }),
        "the unbound project's supervisor is gone"
    );
    for _ in 0..(daemon::search_lifecycle_owner::MAINTENANCE_TENURE_SLICES + 2) {
        assert!(matches!(
            owner.run_slice(&slice_budget()),
            SliceOutcome::Current
        ));
    }
    assert!(
        owner.maintenance().is_some(),
        "a lone tenant is not rotated"
    );

    roster.lock().unwrap().clear();
    let SliceOutcome::RotateMaintenance(handle) = owner.run_slice(&slice_budget()) else {
        panic!("an emptied roster hands the supervisor back");
    };
    owner.stop_maintenance(&handle).await.unwrap();
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    assert!(owner.maintenance().is_none());
    owner.shutdown().await.unwrap();
}

/// AC8, S3, S4: shutdown while the supervisor holds a native call waits for the grace and reports the drain unresolved without dropping the manager, then joins once the call returns; a disable while maintenance runs closes admission first and reconciles through the same supervisor, so nothing starts a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn shutdown_and_disable_join_the_running_supervisor() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("held-row", "held text");
    records(home);
    let engine = TestEngine::new();
    let held = engine.block_calls();
    let scope = ProjectScope::new(PROJECT).unwrap();
    let make_owner = || {
        let scope = scope.clone();
        let owner = SearchLifecycleOwner::for_home(
            home,
            Arc::clone(&corpus.kernel),
            component(&engine, LocalEmbeddingsLimits::default()),
        )
        .with_roster(Arc::new(move || {
            vec![("project:a".to_owned(), scope.clone())]
        }));
        owner.set_drain_grace_for_test(Duration::from_millis(300));
        owner
    };
    let owner = make_owner();
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    for _ in 0..2 {
        let _ = owner.run_slice(&slice_budget());
    }
    let outcome = owner.run_slice(&slice_budget());
    assert!(matches!(outcome, SliceOutcome::Current), "{outcome:?}");
    let started = Instant::now();
    while engine.calls() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the supervisor submits the row"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let unresolved = owner.shutdown().await;
    assert!(unresolved.is_err(), "a held native call outlives the grace");
    assert!(
        owner.maintenance().is_some(),
        "an unresolved supervisor keeps its manager and task"
    );
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Disabled
    ));
    TestEngine::release(&held);
    owner.shutdown().await.unwrap();
    assert!(
        owner.maintenance().is_none(),
        "the released call let the supervisor join"
    );

    let owner = make_owner();
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    assert!(owner.maintenance().is_some());
    let mut events = Vec::new();
    owner
        .disable(&slice_budget(), &mut |event| events.push(event))
        .await
        .unwrap();
    assert!(matches!(
        events.first(),
        Some(DisableEvent::AdmissionClosed)
    ));
    assert!(events.contains(&DisableEvent::WorkersJoined));
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Disabled
    ));
    assert!(owner.maintenance().is_none());
}
