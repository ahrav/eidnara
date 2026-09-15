//! Exercises the daemon's lifecycle owner from a recorded request through selection, completion, pinned reads, disable, authorized recovery, and restart, then the same convergence through a running daemon.

mod support;

use std::num::{NonZeroU64, NonZeroUsize};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daemon::projection_admission::{ADMISSION_DIR, EVIDENCE_RECORD, InputRefusal};
use daemon::projection_gates::{Denial, EntryPoint, ProjectionHook};
use daemon::projection_lifecycle::{
    Cause, ConsumerBinding, ControlState, LifecycleRequest, ProjectionLifecycle, Transition,
};
use daemon::search_catchup::{
    Blocked, CatchUpConsumer, EpisodeBounds, EpisodeEnd, EpisodeEvent, SearchCatchUp,
};
use daemon::search_lifecycle_owner::{
    IDENTITY_CONTRACT_VERSION, PROJECTION_POLICY_VERSION, SearchLifecycleOwner, SliceEvent,
    SliceOutcome, SpecRefusal,
};
use daemon::search_replacement::BuildError;
use daemon::search_replacement::selection::disable::DisableEvent;
use daemon::search_replacement::selection::recovery::RecoveryProgress;
use host_runtime::lifecycle::LifecycleTransactionLock;
use host_runtime::local_embeddings::{LocalEmbeddingsComponent, LocalEmbeddingsLimits};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CommitPageBounds, KernelStore, ProjectScope, SourceHoldAdmission, SourceHoldBinding,
    SourcePageBounds,
};
use retrieval::PersistBounds;
use retrieval::batch::BatchBounds;
use serde_json::Value;
use support::embedding_fixtures::{
    Corpus, GENERATION, TestEngine, budget, component, identity, kernel_incarnation_id,
};
use support::kernel_daemon::KernelDaemon;
use support::projection_gate::{
    LAG_LIMIT, LIMIT, campaign_json, manifest_json, manifest_json_with, write_records,
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
    let outcome = owner.run_slice(&EvalBudget::unbounded());
    assert!(
        matches!(outcome, SliceOutcome::Advanced(RecoveryProgress::Selected)),
        "{outcome:?}"
    );
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

/// A catch-up window with more than `export_page_rows` rows spans as many export pages as its rows need and reaches the target.
#[test]
fn a_catch_up_window_spans_the_pages_its_rows_need() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(&identity, &ProjectionHook::ALL, &[("export_page_rows", 1)]),
        &campaign_json(&identity),
    );
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

    for index in 0..2 {
        corpus.publish(&format!("later-{index}"), "later text");
    }
    let outcome = owner.run_slice(&slice_budget());
    let SliceOutcome::CaughtUp(report) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(report.end, EpisodeEnd::ReachedTarget, "{report:?}");
    assert_eq!(report.acknowledged_through, corpus.tip());
}

/// The owner refuses a rebuild request for another kernel incarnation without creating a control record and accepts the following request for its own incarnation.
#[test]
fn a_rebuild_request_naming_another_kernel_incarnation_records_nothing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());

    let mut foreign = rebuild(home);
    foreign.kernel_incarnation_id = "another-kernel".to_owned();
    assert!(
        owner.request(&foreign, now(), &slice_budget()).is_err(),
        "a request for another kernel is refused"
    );
    assert!(matches!(control(home), ControlState::Absent));
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .expect("the refused request left nothing to conflict with");
    assert!(matches!(control(home), ControlState::Intent(_)));
}

/// With an unbounded caller budget, `supervisor_slice_ms` bounds the slice's wait for a kernel reader.
#[test]
fn a_slice_bounds_its_kernel_reader_wait_by_the_manifest_deadline() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("supervisor_slice_ms", 500)],
        ),
        &campaign_json(&identity),
    );
    let owner = owner(home, &corpus.kernel);
    let held = std::sync::Barrier::new(2);
    let waited = std::thread::scope(|scope| {
        scope.spawn(|| {
            corpus
                .kernel
                .hold_readers_for_test(&held, Duration::from_secs(4))
        });
        held.wait();
        let started = Instant::now();
        let outcome = owner.run_slice(&EvalBudget::unbounded());
        (started.elapsed(), outcome)
    });
    let (waited, outcome) = waited;
    assert!(
        waited < Duration::from_secs(2),
        "the slice waited {waited:?} for a kernel reader: {outcome:?}"
    );
    assert!(
        matches!(outcome, SliceOutcome::Closed(SpecRefusal::Kernel(_))),
        "{outcome:?}"
    );
}

/// A second handler on the same data home opens the kernel once the first one shuts down, while the first handler stays alive with a lifecycle owner bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn shutdown_releases_the_owners_kernel_lease_to_a_successor() {
    use daemon::kernel_routes::KernelState;
    use host_runtime::{CompositeComponent, HostInit, PrimaryComponent};

    let first = KernelDaemon::start().await;
    let started = Instant::now();
    while first.handler().search_lifecycle().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no owner bound"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let descriptor = daemon::dev_descriptor_at(first.data_home().to_str().unwrap());
    let second = daemon::Handler::new();
    second.disable_kernel_sampler_for_test();
    PrimaryComponent::initialize(
        &second,
        HostInit {
            host_capabilities: Vec::new(),
            storage: Some(serde_json::to_value(&descriptor).unwrap()),
        },
    )
    .await
    .unwrap();
    PrimaryComponent::activate(&second).await.unwrap();

    CompositeComponent::shutdown(first.handler()).await.unwrap();
    let started = Instant::now();
    while second.kernel_state() != KernelState::Ready {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the successor stayed {:?} while the first handler is alive",
            second.kernel_state()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    CompositeComponent::shutdown(&second).await.unwrap();
    drop(first);
}

fn episode_bounds() -> EpisodeBounds {
    let admission = SourceHoldAdmission {
        max_references: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
    };
    EpisodeBounds {
        commits: CommitPageBounds {
            max_commits: NonZeroUsize::new(64).unwrap(),
            max_rows: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        },
        hold_admission: admission,
        source_page: SourcePageBounds {
            max_rows: NonZeroUsize::new(64).unwrap(),
            max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
            max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
            max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
        },
        max_source_pages: NonZeroUsize::new(8).unwrap(),
        max_source_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        batch: BatchBounds {
            persist: PersistBounds {
                max_records: NonZeroUsize::new(64).unwrap(),
                max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
                max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
            },
            max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_local_mutations: NonZeroUsize::new(64).unwrap(),
            max_pending: NonZeroUsize::new(64).unwrap(),
        },
    }
}

fn current_owner(home: &Path, corpus: &Corpus) -> SearchLifecycleOwner {
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
    owner
}

/// An at-tip family whose kernel acknowledgement trails its local prefix is acknowledged by the next slice even with no new commits.
#[test]
fn an_at_tip_family_still_reconciles_a_lost_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);

    corpus.publish("later", "later text");
    // The window commits locally and the budget ends before the acknowledgement.
    {
        let budget = slice_budget();
        let reader = owner.pin(&budget).unwrap();
        let checkpoint = reader.coverage(&budget).unwrap().checkpoint;
        let consumer = CatchUpConsumer {
            binding: SourceHoldBinding {
                consumer_id: reader.consumer().consumer_id.clone(),
                lease_epoch: corpus.kernel.lease_epoch(),
                source_policy_version: PROJECTION_POLICY_VERSION.to_owned(),
            },
            hold_id: checkpoint.hold_id,
            kernel_incarnation_id: kernel_incarnation_id(home),
            generation_id: Some(reader.consumer().generation_id.clone()),
        };
        let report = SearchCatchUp::new(&corpus.kernel, reader.projection())
            .with_budget(budget.clone())
            .run_episode(&consumer, &episode_bounds(), now(), &mut |event| {
                if matches!(event, EpisodeEvent::LocalReleased { .. }) {
                    budget.cancel();
                }
            })
            .unwrap();
        assert_eq!(report.end, EpisodeEnd::Blocked(Blocked::Cancelled));
    }
    let local = owner
        .pin(&slice_budget())
        .unwrap()
        .coverage(&slice_budget())
        .unwrap()
        .checkpoint
        .checkpoint_commit_seq;
    assert_eq!(local, corpus.tip());
    let acknowledged = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
    assert!(acknowledged < Some(local), "{acknowledged:?} >= {local}");

    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::CaughtUp(report) if report.acknowledged_through == local),
        "{outcome:?}"
    );
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        Some(local)
    );
}

/// A rebuild recorded over a Current family whose supervisor runs hands that supervisor back before the replacement is built, and then completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_rebuild_over_a_maintained_family_stops_maintenance_first() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("a-row", "alpha text");
    records(home);
    let owner = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
        .with_roster(Arc::new(|| {
            vec![("project:a".to_owned(), ProjectScope::new(PROJECT).unwrap())]
        }));
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
    drive(&owner, 40, || published(&owner).len() == 1).await;
    assert!(
        owner.maintenance().is_some(),
        "the lone project is maintained"
    );
    let ControlState::Current(current) = control(home) else {
        panic!("the first rebuild reached Current");
    };

    let mut again = rebuild(home);
    again.selected_generation = current.staged_seed_digest.clone().unwrap();
    again.consumer.consumer_id = "search-lifecycle-again".to_owned();
    again.attempt_id = "rebuild-again".to_owned();
    owner.request(&again, now(), &slice_budget()).unwrap();
    let slices = drive(&owner, 12, || {
        matches!(control(home), ControlState::Current(done) if done.attempt_id == "rebuild-again")
    })
    .await;
    assert!(slices < 12, "the second rebuild reached Current");
    owner.shutdown().await.unwrap();
}

/// A lone project's supervisor is handed back once its episode grant expires, so a row created after `B_recovery_ms` is still embedded.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_lone_supervisor_is_renewed_when_its_grant_expires() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("a-row", "alpha text");
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(&identity, &ProjectionHook::ALL, &[("B_recovery_ms", 3_000)]),
        &campaign_json(&identity),
    );
    let owner = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
        .with_roster(Arc::new(|| {
            vec![("project:a".to_owned(), ProjectScope::new(PROJECT).unwrap())]
        }));
    let _ = owner.run_slice(&slice_budget());
    let mut short = rebuild(home);
    short.deadline = now() + 3_000;
    owner.request(&short, now(), &slice_budget()).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            owner.run_slice(&slice_budget()),
            SliceOutcome::Advanced(_)
        ));
    }
    drive(&owner, 40, || published(&owner).len() == 1).await;
    let first = owner.maintenance().expect("the lone project is maintained");

    tokio::time::sleep(Duration::from_millis(3_200)).await;
    let later = corpus.publish("later", "later text");
    let slices = drive(&owner, 30, || published(&owner).len() == 2).await;
    assert!(
        slices < 30,
        "the later row was embedded: {:?}",
        published(&owner)
    );
    assert!(published(&owner).contains(&later));
    let renewed = owner.maintenance().expect("a supervisor is running");
    assert_eq!(renewed.scope, first.scope);
    owner.shutdown().await.unwrap();
}

/// A Current family under one-commit windows with `count` later rows, so a catch-up episode crosses many boundaries.
fn owner_with_many_windows(home: &Path, corpus: &Corpus, count: usize) -> SearchLifecycleOwner {
    corpus.publish("kept", "kept text");
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("catchup_batch_commits", 1), ("catchup_lag_commits", 1_000)],
        ),
        &campaign_json(&identity),
    );
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
    for index in 0..count {
        corpus.publish(&format!("later-{index}"), "later text");
    }
    owner
}

/// Pauses the episode at the second window's first boundary: the returned receiver fires once, then the episode waits for one message on the returned sender. Both waits are bounded, so a missing window fails the test instead of hanging it.
fn pause_at_second_window(
    owner: &SearchLifecycleOwner,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    let windows = std::sync::atomic::AtomicUsize::new(0);
    owner.tap_slice_events_for_test(move |event| {
        if matches!(
            event,
            SliceEvent::Episode(EpisodeEvent::HoldExtensionRequested { .. })
        ) && windows.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
        {
            let _ = reached_tx.send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
    });
    (reached_rx, release_tx)
}

const PAUSE_WAIT: Duration = Duration::from_secs(20);

/// Closing the gate cancels a running catch-up episode before it acknowledges every pending window.
#[test]
fn a_revoked_grant_stops_a_running_catch_up_episode() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = owner_with_many_windows(home, &corpus, 8);
    let tip = corpus.tip();
    let (reached, release) = pause_at_second_window(&owner);

    let outcome = std::thread::scope(|scope| {
        let slice = scope.spawn(|| owner.run_slice(&slice_budget()));
        reached
            .recv_timeout(PAUSE_WAIT)
            .expect("the episode reached the second window");
        owner.admission().gate().close();
        release.send(()).expect("the paused episode is waiting");
        slice.join().unwrap()
    });
    let SliceOutcome::CaughtUp(report) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        report.end,
        EpisodeEnd::Blocked(Blocked::Cancelled),
        "{report:?}"
    );
    assert_eq!(report.batches_applied, 1, "{report:?}");
    assert!(report.acknowledged_through < tip, "{report:?}");
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        Some(report.acknowledged_through),
        "nothing was acknowledged after the revocation"
    );
}

/// A rebuild request under a manifest whose limits cannot bound a replacement is refused without a control record, and the request records once the manifest can.
#[test]
fn a_rebuild_request_under_unboundable_limits_records_nothing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(&identity, &ProjectionHook::ALL, &[("retry_attempts", 2)]),
        &campaign_json(&identity),
    );
    let owner = owner(home, &corpus.kernel);
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Unregistered
    ));
    assert!(
        owner
            .request(&rebuild(home), now(), &slice_budget())
            .is_err(),
        "limits that refuse every slice refuse the request"
    );
    assert!(matches!(control(home), ControlState::Absent));

    records(home);
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    assert!(matches!(control(home), ControlState::Intent(_)));
}

/// Disabling a home that has run a slice but registered no family persists the stop and completes; there is no family to reconcile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_an_unregistered_home_completes() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Unregistered
    ));
    owner
        .disable(&slice_budget(), &mut |_| {})
        .await
        .expect("nothing to reconcile");
    assert!(matches!(control(home), ControlState::Disabled(_)));
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Disabled
    ));
}

/// A rebuild request whose deadline lies further out than its transition's bound is refused without a control record.
#[test]
fn a_rebuild_request_past_its_transition_bound_records_nothing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());
    let mut far = rebuild(home);
    far.deadline = now() + i64::try_from(LIMIT).unwrap() + 10_000;
    assert!(
        owner.request(&far, now(), &slice_budget()).is_err(),
        "a deadline past `B_recovery_ms` is refused"
    );
    assert!(matches!(control(home), ControlState::Absent));
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    assert!(matches!(control(home), ControlState::Intent(_)));
}

/// A manifest that lowers the coverage bounds without changing the identity takes effect on the next slice: the selected family is judged and pinned under the new bounds.
#[test]
fn a_manifest_that_changes_the_coverage_bounds_refreshes_the_selection() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(&identity, &ProjectionHook::ALL, &[("export_page_rows", 2)]),
        &campaign_json(&identity),
    );
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(
            outcome,
            SliceOutcome::Current | SliceOutcome::Advanced(RecoveryProgress::Current)
        ),
        "{outcome:?}"
    );
    owner
        .pin(&slice_budget())
        .expect("the family is admitted under the new bounds");
}

/// A slice that hands the supervisor back because its inputs were refused closes admission first, so nothing runs under the old records while the supervisor drains.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_refused_manifest_closes_admission_before_maintenance_drains() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("a-row", "alpha text");
    records(home);
    let owner = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
        .with_roster(Arc::new(|| {
            vec![("project:a".to_owned(), ProjectScope::new(PROJECT).unwrap())]
        }));
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
    drive(&owner, 40, || published(&owner).len() == 1).await;
    assert!(owner.maintenance().is_some());

    std::fs::remove_dir_all(home.join(ADMISSION_DIR)).unwrap();
    let outcome = owner.run_slice(&slice_budget());
    let SliceOutcome::RotateMaintenance(handle) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::NoManifest,
        "the old records admit nothing once they are gone"
    );
    assert!(owner.pin(&slice_budget()).is_err());
    owner.stop_maintenance(&handle).await.unwrap();
    owner.shutdown().await.unwrap();
}

/// A Current family whose catch-up is blocked on a dead hold still rotates its supervisor when the roster changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_blocked_catch_up_still_reconciles_maintenance() {
    use support::embedding_fixtures::{PROJECT, PROJECT_B};
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("a-row", "alpha text");
    records(home);
    let roster = Arc::new(std::sync::Mutex::new(vec![(
        "project:a".to_owned(),
        ProjectScope::new(PROJECT).unwrap(),
    )]));
    let owner = {
        let roster = Arc::clone(&roster);
        SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
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
    let slices = drive(&owner, 40, || published(&owner).len() == 1).await;
    assert!(slices < 40, "the supervisor published the row");
    owner.shutdown().await.unwrap();
    drop(owner);

    // The restarted owner's dead hold blocks catch-up while the family continues serving.
    drop(corpus);
    let corpus = Corpus::open(home);
    let owner = {
        let roster = Arc::clone(&roster);
        SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
            .with_roster(Arc::new(move || roster.lock().unwrap().clone()))
    };
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    let first = owner.maintenance().expect("the lone project is maintained");
    assert_eq!(*first.scope, ProjectScope::new(PROJECT).unwrap());
    corpus.publish("later", "later text");
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::CaughtUp(report) if matches!(report.end, EpisodeEnd::Blocked(Blocked::HoldExtension(_)))),
        "{outcome:?}"
    );

    *roster.lock().unwrap() = vec![(
        "project:b".to_owned(),
        ProjectScope::new(PROJECT_B).unwrap(),
    )];
    let outcome = owner.run_slice(&slice_budget());
    let SliceOutcome::RotateMaintenance(handle) = outcome else {
        panic!("a project off the roster loses its supervisor: {outcome:?}");
    };
    owner.stop_maintenance(&handle).await.unwrap();
    owner.shutdown().await.unwrap();
}

/// Cancelling the caller's budget while a catch-up episode waits for a kernel reader ends the slice promptly.
#[test]
fn caller_cancellation_interrupts_a_catch_up_waiting_for_a_kernel_reader() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = owner_with_many_windows(home, &corpus, 8);
    let (reached, release) = pause_at_second_window(&owner);

    let budget = slice_budget();
    let held = std::sync::Barrier::new(2);
    let (waited, outcome) = std::thread::scope(|scope| {
        let slice = {
            let budget = budget.clone();
            scope.spawn(move || owner.run_slice(&budget))
        };
        reached
            .recv_timeout(PAUSE_WAIT)
            .expect("the episode reached the second window");
        // The readers are held before the episode resumes, so its next kernel read waits on the pool.
        scope.spawn(|| {
            corpus
                .kernel
                .hold_readers_for_test(&held, Duration::from_secs(3))
        });
        held.wait();
        release.send(()).expect("the paused episode is waiting");
        std::thread::sleep(Duration::from_millis(150));
        let cancelled = Instant::now();
        budget.cancel();
        let outcome = slice.join().unwrap();
        (cancelled.elapsed(), outcome)
    });
    assert!(
        waited < Duration::from_millis(1_500),
        "the slice ran {waited:?} after cancellation: {outcome:?}"
    );
    assert!(
        matches!(&outcome, SliceOutcome::CaughtUp(report) if report.end == EpisodeEnd::Blocked(Blocked::Cancelled)),
        "{outcome:?}"
    );
}

/// A manifest that lowers a maintenance limit without changing the identity or coverage bounds hands the running supervisor back so the next one starts under the new bounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_manifest_that_changes_the_maintenance_bounds_rotates_the_supervisor() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("a-row", "alpha text");
    records(home);
    let owner = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
        .with_roster(Arc::new(|| {
            vec![("project:a".to_owned(), ProjectScope::new(PROJECT).unwrap())]
        }));
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
    drive(&owner, 40, || published(&owner).len() == 1).await;
    assert!(owner.maintenance().is_some());

    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(&identity, &ProjectionHook::ALL, &[("pending_count", 100)]),
        &campaign_json(&identity),
    );
    let outcome = owner.run_slice(&slice_budget());
    let SliceOutcome::RotateMaintenance(handle) = outcome else {
        panic!("changed maintenance bounds hand the supervisor back: {outcome:?}");
    };
    owner.stop_maintenance(&handle).await.unwrap();
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    assert!(
        owner.maintenance().is_some(),
        "a supervisor restarts under the new bounds"
    );
    owner.shutdown().await.unwrap();
}

/// A manifest that lowers `B_recovery_ms` alone hands the running supervisor back: its episode grant would otherwise keep the older, later deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_manifest_that_lowers_the_recovery_bound_rotates_the_supervisor() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("a-row", "alpha text");
    records(home);
    let owner = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
        .with_roster(Arc::new(|| {
            vec![("project:a".to_owned(), ProjectScope::new(PROJECT).unwrap())]
        }));
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
    drive(&owner, 40, || published(&owner).len() == 1).await;
    assert!(owner.maintenance().is_some());

    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("B_recovery_ms", 500_000)],
        ),
        &campaign_json(&identity),
    );
    let outcome = owner.run_slice(&slice_budget());
    let SliceOutcome::RotateMaintenance(handle) = outcome else {
        panic!("a lowered recovery bound hands the supervisor back: {outcome:?}");
    };
    owner.stop_maintenance(&handle).await.unwrap();
    owner.shutdown().await.unwrap();
}

/// Holds the selected family's projection connection on another thread for `hold`, so anything reading the projection waits.
fn hold_projection<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    reader: &'scope daemon::search_replacement::selection::SearchReader,
    held: &'scope std::sync::Barrier,
    hold: Duration,
) {
    scope.spawn(move || {
        reader
            .read(&budget(Duration::from_secs(30)), |_| {
                held.wait();
                std::thread::sleep(hold);
                Ok(())
            })
            .unwrap();
    });
}

/// A slice on an active record whose deadline is nearer than the slice bound ends by that deadline even while the projection connection is held.
#[test]
fn a_slice_ends_by_the_records_deadline_while_the_projection_is_held() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let ControlState::Current(current) = control(home) else {
        panic!("the rebuild reached Current");
    };
    let mut again = rebuild(home);
    again.selected_generation = current.staged_seed_digest.clone().unwrap();
    again.consumer.consumer_id = "search-lifecycle-again".to_owned();
    again.attempt_id = "rebuild-again".to_owned();
    again.deadline = now() + 2_500;
    owner.request(&again, now(), &slice_budget()).unwrap();

    let reader = owner.pin(&slice_budget()).unwrap();
    let held = std::sync::Barrier::new(2);
    let (waited, outcome) = std::thread::scope(|scope| {
        hold_projection(scope, &reader, &held, Duration::from_secs(6));
        held.wait();
        let started = Instant::now();
        let outcome = owner.run_slice(&slice_budget());
        (started.elapsed(), outcome)
    });
    assert!(
        waited < Duration::from_secs(4),
        "the slice ran {waited:?} on a record with 2.5 s left: {outcome:?}"
    );
    assert!(matches!(outcome, SliceOutcome::Blocked(_)), "{outcome:?}");
}

/// A pin with a short budget returns within that budget while a slice holds the owner across a long projection read.
#[test]
fn a_pin_waits_for_the_owner_only_within_its_budget() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let reader = owner.pin(&slice_budget()).unwrap();
    let held = std::sync::Barrier::new(2);
    let (waited, pinned) = std::thread::scope(|scope| {
        hold_projection(scope, &reader, &held, Duration::from_secs(4));
        held.wait();
        // The slice takes `managed` and then waits for the held projection connection.
        scope.spawn(|| owner.run_slice(&slice_budget()));
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        let pinned = owner.pin(&budget(Duration::from_millis(300)));
        (started.elapsed(), pinned.map(|_| ()))
    });
    assert!(
        waited < Duration::from_secs(2),
        "the pin waited {waited:?} for the owner: {pinned:?}"
    );
    assert!(pinned.is_err(), "{pinned:?}");
}

/// A catch-up episode that applied batches is reported as such even when maintenance cannot start afterwards, so the loop reschedules without idling.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn catch_up_progress_is_reported_when_maintenance_cannot_start() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    let identity = identity(&kernel_incarnation_id(home));
    let without_backfill: Vec<ProjectionHook> = ProjectionHook::ALL
        .iter()
        .copied()
        .filter(|hook| *hook != ProjectionHook::EmbeddingBackfill)
        .collect();
    write_records(
        home,
        &manifest_json(&identity, &without_backfill),
        &campaign_json(&identity),
    );
    let owner = SearchLifecycleOwner::for_home(home, Arc::clone(&corpus.kernel), lane())
        .with_roster(Arc::new(|| {
            vec![("project:a".to_owned(), ProjectScope::new(PROJECT).unwrap())]
        }));
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
    corpus.publish("later", "later text");
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::CaughtUp(report) if report.batches_applied >= 1 && report.end == EpisodeEnd::ReachedTarget),
        "{outcome:?}"
    );
    assert!(owner.maintenance().is_none(), "backfill is disabled");
    owner.shutdown().await.unwrap();
}

/// A slice on a record with 600 ms remaining returns a deadline block while a projection connection is held.
#[test]
fn a_slice_inside_the_deadline_margin_is_bounded_by_the_record_deadline() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let ControlState::Current(current) = control(home) else {
        panic!("the rebuild reached Current");
    };
    let mut again = rebuild(home);
    again.selected_generation = current.staged_seed_digest.clone().unwrap();
    again.consumer.consumer_id = "search-lifecycle-again".to_owned();
    again.attempt_id = "rebuild-again".to_owned();
    again.deadline = now() + 600;
    owner.request(&again, now(), &slice_budget()).unwrap();

    let reader = owner.pin(&slice_budget()).unwrap();
    let held = std::sync::Barrier::new(2);
    let (waited, outcome) = std::thread::scope(|scope| {
        hold_projection(scope, &reader, &held, Duration::from_secs(5));
        held.wait();
        let started = Instant::now();
        let outcome = owner.run_slice(&slice_budget());
        (started.elapsed(), outcome)
    });
    assert!(
        waited < Duration::from_secs(2),
        "the slice ran {waited:?} on a record with 600 ms left: {outcome:?}"
    );
    assert!(
        matches!(&outcome, SliceOutcome::Blocked(reason) if reason.contains("deadline")),
        "{outcome:?}"
    );
}

/// A rebuild request records an `Explicit` admission entry.
#[test]
fn a_rebuild_request_is_admitted_as_an_explicit_action() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());
    owner
        .request(&rebuild(home), now(), &slice_budget())
        .unwrap();
    let entries: Vec<EntryPoint> = owner
        .admission()
        .gate()
        .ledger()
        .iter()
        .filter(|entry| entry.hook == ProjectionHook::EmbeddingBootstrap && entry.verdict.is_ok())
        .map(|entry| entry.entry)
        .collect();
    assert!(
        entries.contains(&EntryPoint::Explicit),
        "the request's admission is recorded as explicit: {entries:?}"
    );
}

/// A request made after the admission records are gone is refused and closes the gate, even though an earlier slice installed evidence.
#[test]
fn a_request_without_current_records_is_refused_and_closes_admission() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let ControlState::Current(current) = control(home) else {
        panic!("the rebuild reached Current");
    };
    let mut again = rebuild(home);
    again.selected_generation = current.staged_seed_digest.clone().unwrap();
    again.consumer.consumer_id = "search-lifecycle-again".to_owned();
    again.attempt_id = "rebuild-again".to_owned();

    std::fs::remove_dir_all(home.join(ADMISSION_DIR)).unwrap();
    assert!(
        matches!(
            owner.request(&again, now(), &slice_budget()),
            Err(BuildError::Intent(_))
        ),
        "withdrawn records refuse the request"
    );
    assert!(
        matches!(control(home), ControlState::Current(done) if done.attempt_id == current.attempt_id),
        "nothing was recorded"
    );
    assert_eq!(
        owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Reload)
            .unwrap_err(),
        Denial::NoManifest
    );
}

/// A window whose rows were each created and invalidated inside it applies in one batch: two mutations per source row fit the mutation bound.
#[test]
fn a_window_of_rows_created_and_invalidated_within_it_catches_up() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[
                ("local_transaction_bytes", 131_200),
                ("catchup_lag_commits", 1_000),
            ],
        ),
        &campaign_json(&identity),
    );
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

    // Five created-and-invalidated rows produce 10 mutations against an eight-row batch.
    let objects: Vec<String> = (0..5)
        .map(|index| corpus.publish(&format!("brief-{index}"), "brief text"))
        .collect();
    for object in &objects {
        corpus.retire(object);
    }
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::CaughtUp(report) if report.end == EpisodeEnd::ReachedTarget && report.acknowledged_through == corpus.tip()),
        "{outcome:?}"
    );
}

/// A window is never wider than the rows one batch admits, so a run of single-row commits beyond the batch capacity splits across windows and catches up.
#[test]
fn commit_pages_are_bounded_by_the_batch_row_capacity() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[
                ("local_transaction_bytes", 131_200),
                ("catchup_lag_commits", 1_000),
            ],
        ),
        &campaign_json(&identity),
    );
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

    // Nine rows against an eight-row batch.
    for index in 0..9 {
        corpus.publish(&format!("many-{index}"), "many text");
    }
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::CaughtUp(report) if report.end == EpisodeEnd::ReachedTarget && report.acknowledged_through == corpus.tip() && report.batches_applied >= 2),
        "{outcome:?}"
    );
}

/// The evidence a slice installs and the bounds it runs under come from one manifest read: a manifest replaced mid-slice takes effect on the next slice.
#[test]
fn a_slice_installs_the_manifest_it_derived_its_bounds_from() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let identity = identity(&kernel_incarnation_id(home));
    // The slice pauses after it has read the records and before it refreshes admission.
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    let paused = std::sync::atomic::AtomicBool::new(false);
    owner.tap_slice_events_for_test(move |event| {
        if matches!(event, SliceEvent::Prepared { .. })
            && !paused.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let _ = reached_tx.send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
    });
    let outcome = std::thread::scope(|scope| {
        let slice = scope.spawn(|| owner.run_slice(&slice_budget()));
        reached
            .recv_timeout(PAUSE_WAIT)
            .expect("the slice read the records");
        write_records(
            home,
            &manifest_json_with(
                &identity,
                &ProjectionHook::ALL,
                &[("catchup_batch_source_bytes", 1_024)],
            ),
            &campaign_json(&identity),
        );
        release.send(()).expect("the paused slice is waiting");
        slice.join().unwrap()
    });
    assert!(matches!(outcome, SliceOutcome::Current), "{outcome:?}");
    let gate = owner.admission().gate();
    let grant = gate
        .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Dispatch)
        .unwrap();
    let expected = daemon::projection_gates::InvalidationIdentity::from(&identity);
    assert!(
        gate.check_limits(&grant, &expected, &[("catchup_batch_source_bytes", LIMIT)])
            .is_ok(),
        "the slice installed the manifest it read at its start"
    );
    let _ = owner.run_slice(&slice_budget());
    let grant = gate
        .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Dispatch)
        .unwrap();
    assert!(
        gate.check_limits(&grant, &expected, &[("catchup_batch_source_bytes", LIMIT)])
            .is_err(),
        "the next slice installs the replaced manifest"
    );
}

/// A rebuild over a family that trails the target by more commit pages than a window has source pages still retires the old consumer and completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn retirement_verifies_a_long_commit_span_page_by_page() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let first = owner_with_many_windows(home, &corpus, 0);
    first.shutdown().await.unwrap();
    drop(first);

    drop(corpus);
    let corpus = Corpus::open(home);
    let owner = owner(home, &corpus.kernel);
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Current
    ));
    // The dead hold blocks catch-up, so the family trails the target commits.
    for index in 0..50 {
        corpus.publish(&format!("trail-{index}"), "trail text");
    }
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::CaughtUp(report) if matches!(report.end, EpisodeEnd::Blocked(Blocked::HoldExtension(_)))),
        "{outcome:?}"
    );
    let ControlState::Current(current) = control(home) else {
        panic!("the first rebuild reached Current");
    };
    let mut again = rebuild(home);
    again.selected_generation = current.staged_seed_digest.clone().unwrap();
    again.consumer.consumer_id = "search-lifecycle-again".to_owned();
    again.attempt_id = "rebuild-again".to_owned();
    owner.request(&again, now(), &slice_budget()).unwrap();
    let slices = drive(&owner, 20, || {
        matches!(control(home), ControlState::Current(done) if done.attempt_id == "rebuild-again")
    })
    .await;
    assert!(slices < 20, "the second rebuild reached Current");
    owner.shutdown().await.unwrap();
}

/// A request made after a manifest reload disabled its hook is refused, even though the previous slice's evidence admitted it.
#[test]
fn a_request_is_judged_on_the_records_it_reads_not_on_cached_evidence() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let ControlState::Current(current) = control(home) else {
        panic!("the rebuild reached Current");
    };
    let identity = identity(&kernel_incarnation_id(home));
    let without_bootstrap: Vec<ProjectionHook> = ProjectionHook::ALL
        .iter()
        .copied()
        .filter(|hook| *hook != ProjectionHook::EmbeddingBootstrap)
        .collect();
    write_records(
        home,
        &manifest_json(&identity, &without_bootstrap),
        &campaign_json(&identity),
    );
    let mut again = rebuild(home);
    again.selected_generation = current.staged_seed_digest.clone().unwrap();
    again.consumer.consumer_id = "search-lifecycle-again".to_owned();
    again.attempt_id = "rebuild-again".to_owned();
    assert!(
        matches!(
            owner.request(&again, now(), &slice_budget()),
            Err(BuildError::Intent(_))
        ),
        "a disabled hook refuses the request"
    );
    assert!(
        matches!(control(home), ControlState::Current(done) if done.attempt_id == current.attempt_id),
        "nothing was recorded"
    );
}

/// Builds a Current family for `PROJECT` whose lone supervisor holds a native call the test controls.
fn owner_with_held_maintenance(
    home: &Path,
    corpus: &Corpus,
    engine: &Arc<TestEngine>,
) -> SearchLifecycleOwner {
    use support::embedding_fixtures::PROJECT;
    corpus.publish("held-row", "held text");
    records(home);
    let scope = ProjectScope::new(PROJECT).unwrap();
    let owner = SearchLifecycleOwner::for_home(
        home,
        Arc::clone(&corpus.kernel),
        component(engine, LocalEmbeddingsLimits::default()),
    )
    .with_roster(Arc::new(move || {
        vec![("project:a".to_owned(), scope.clone())]
    }));
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
        std::thread::sleep(Duration::from_millis(20));
    }
    owner
}

/// Runs `disable` on its own thread and reports its events; the thread ends when the disable returns.
fn disable_on_a_thread(
    owner: &Arc<SearchLifecycleOwner>,
    budget: EvalBudget,
) -> (
    std::thread::JoinHandle<Result<(), BuildError>>,
    std::sync::mpsc::Receiver<DisableEvent>,
) {
    let (events, received) = std::sync::mpsc::channel();
    let owner = Arc::clone(owner);
    let thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(owner.disable(&budget, &mut |event| {
                let _ = events.send(event);
            }))
    });
    (thread, received)
}

/// A shutdown that overlaps a disable waits for it within the drain grace and reports the disable unresolved otherwise; a resolved disable lets a later shutdown complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn shutdown_waits_for_an_in_flight_disable() {
    use daemon::search_lifecycle_owner::ShutdownUnresolved;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let engine = TestEngine::new();
    let held = engine.block_calls();
    let _release_on_unwind = support::embedding_fixtures::GateGuard(Arc::clone(&held));
    let owner = Arc::new(owner_with_held_maintenance(home, &corpus, &engine));
    owner.set_drain_grace_for_test(Duration::from_millis(300));

    // The disable joins the supervisor, whose native call is held, so it stays in reconciliation.
    let (disabling, events) = disable_on_a_thread(&owner, slice_budget());
    let started = Instant::now();
    loop {
        let event = events
            .recv_timeout(Duration::from_secs(10))
            .expect("the disable reaches its drain");
        if event == DisableEvent::DrainStarted {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    let outcome = owner.shutdown().await;
    assert!(
        matches!(outcome, Err(ShutdownUnresolved::Disabling)),
        "a disable still reconciling is not a resolved shutdown: {outcome:?}"
    );
    TestEngine::release(&held);
    let _ = disabling.join().unwrap();
    owner.shutdown().await.unwrap();
    assert!(matches!(control(home), ControlState::Disabled(_)));
}

/// A shutdown that waits for a disable and then drains the supervisor it hands back spends one drain grace in total.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn shutdown_spends_one_grace_across_the_disable_wait_and_the_drain() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let engine = TestEngine::new();
    let held = engine.block_calls();
    let _release_on_unwind = support::embedding_fixtures::GateGuard(Arc::clone(&held));
    let owner = Arc::new(owner_with_held_maintenance(home, &corpus, &engine));
    owner.set_drain_grace_for_test(Duration::from_millis(1_500));

    // The disable's own budget ends after 800 ms, so it hands the manager back while the native call is still held.
    let (disabling, events) = disable_on_a_thread(&owner, budget(Duration::from_millis(800)));
    loop {
        let event = events
            .recv_timeout(Duration::from_secs(10))
            .expect("the disable reaches its drain");
        if event == DisableEvent::DrainStarted {
            break;
        }
    }
    let started = Instant::now();
    let outcome = owner.shutdown().await;
    let waited = started.elapsed();
    TestEngine::release(&held);
    let _ = disabling.join().unwrap();
    let _ = owner.shutdown().await;
    assert!(
        matches!(
            outcome,
            Err(daemon::search_lifecycle_owner::ShutdownUnresolved::Drain(_))
        ),
        "the handed-back supervisor is drained and its held call outlives the grace: {outcome:?}"
    );
    assert!(
        waited < Duration::from_millis(1_900),
        "shutdown waited {waited:?} against a 1.5 s grace"
    );
}

/// An active record that expires after selection keeps the evidence a cleanup needs, so a disable that follows reconciles the selected family.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_record_keeps_the_evidence_a_disable_needs() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());
    let mut short = rebuild(home);
    short.deadline = now() + 2_500;
    owner.request(&short, now(), &slice_budget()).unwrap();
    assert!(matches!(
        owner.run_slice(&slice_budget()),
        SliceOutcome::Advanced(RecoveryProgress::Selected)
    ));
    tokio::time::sleep(Duration::from_millis(2_600)).await;
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::Blocked(reason) if reason.contains("deadline")),
        "{outcome:?}"
    );
    owner
        .disable(&slice_budget(), &mut |_| {})
        .await
        .expect("the records still approve the cleanup");
    assert!(matches!(control(home), ControlState::Disabled(_)));
}

/// A refresh that returns inside the deadline margin starts no recovery work and consumes no episode.
#[test]
fn a_refresh_that_ends_inside_the_margin_starts_no_recovery() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let owner = current_owner(home, &corpus);
    let ControlState::Current(current) = control(home) else {
        panic!("the rebuild reached Current");
    };
    let mut again = rebuild(home);
    again.selected_generation = current.staged_seed_digest.clone().unwrap();
    again.consumer.consumer_id = "search-lifecycle-again".to_owned();
    again.attempt_id = "rebuild-again".to_owned();
    again.deadline = now() + 2_500;
    owner.request(&again, now(), &slice_budget()).unwrap();

    let reader = owner.pin(&slice_budget()).unwrap();
    let held = std::sync::Barrier::new(2);
    let outcome = std::thread::scope(|scope| {
        hold_projection(scope, &reader, &held, Duration::from_millis(1_800));
        held.wait();
        owner.run_slice(&slice_budget())
    });
    assert!(
        matches!(&outcome, SliceOutcome::Blocked(reason) if reason == "the record's deadline has passed"),
        "{outcome:?}"
    );
    let ControlState::Intent(intent) = control(home) else {
        panic!("the record stays active");
    };
    assert_eq!(
        intent.episodes.consumed, 0,
        "no episode starts inside the margin"
    );
}

/// A reload that lowers the drain grace and forces a rotation is the grace the next drain uses once the records are gone, even when the first drain did not resolve.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_drain_bound_read_on_the_rotation_path_is_retained() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("held-row", "held text");
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("physical_drain_ms", 4_000)],
        ),
        &campaign_json(&identity),
    );
    let engine = TestEngine::new();
    let held = engine.block_calls();
    let _release_on_unwind = support::embedding_fixtures::GateGuard(Arc::clone(&held));
    let scope = ProjectScope::new(PROJECT).unwrap();
    let owner = SearchLifecycleOwner::for_home(
        home,
        Arc::clone(&corpus.kernel),
        component(&engine, LocalEmbeddingsLimits::default()),
    )
    .with_roster(Arc::new(move || {
        vec![("project:a".to_owned(), scope.clone())]
    }));
    let _ = owner.run_slice(&slice_budget());
    let mut short = rebuild(home);
    short.deadline = now() + 4_000;
    owner.request(&short, now(), &slice_budget()).unwrap();
    for _ in 0..2 {
        let _ = owner.run_slice(&slice_budget());
    }
    let outcome = owner.run_slice(&slice_budget());
    assert!(matches!(outcome, SliceOutcome::Current), "{outcome:?}");
    let started = Instant::now();
    while engine.calls() == 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The reload lowers the grace and changes the coverage bounds, so the slice hands the supervisor back before it prepares.
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("physical_drain_ms", 500), ("export_page_rows", 2)],
        ),
        &campaign_json(&identity),
    );
    let SliceOutcome::RotateMaintenance(handle) = owner.run_slice(&slice_budget()) else {
        panic!("changed bounds hand the supervisor back");
    };
    assert!(owner.stop_maintenance(&handle).await.is_err());
    std::fs::remove_dir_all(home.join(ADMISSION_DIR)).unwrap();
    let SliceOutcome::RotateMaintenance(handle) = owner.run_slice(&slice_budget()) else {
        panic!("refused records hand the supervisor back");
    };
    let started = Instant::now();
    let stopped = owner.stop_maintenance(&handle).await;
    let waited = started.elapsed();
    TestEngine::release(&held);
    let _ = owner.shutdown().await;
    assert!(stopped.is_err(), "{stopped:?}");
    assert!(
        waited < Duration::from_secs(2),
        "the drain waited {waited:?} against the reloaded 500 ms grace"
    );
}

/// A partial reload whose manifest names another identity than the campaign is not an approved grace: a stop keeps the grace the last applicable records approved.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_partial_reload_does_not_change_the_drain_grace() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("held-row", "held text");
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("physical_drain_ms", 4_000)],
        ),
        &campaign_json(&identity),
    );
    let engine = TestEngine::new();
    let held = engine.block_calls();
    let _release_on_unwind = support::embedding_fixtures::GateGuard(Arc::clone(&held));
    let scope = ProjectScope::new(PROJECT).unwrap();
    let owner = SearchLifecycleOwner::for_home(
        home,
        Arc::clone(&corpus.kernel),
        component(&engine, LocalEmbeddingsLimits::default()),
    )
    .with_roster(Arc::new(move || {
        vec![("project:a".to_owned(), scope.clone())]
    }));
    let _ = owner.run_slice(&slice_budget());
    let mut short = rebuild(home);
    short.deadline = now() + 4_000;
    owner.request(&short, now(), &slice_budget()).unwrap();
    for _ in 0..2 {
        let _ = owner.run_slice(&slice_budget());
    }
    let outcome = owner.run_slice(&slice_budget());
    assert!(matches!(outcome, SliceOutcome::Current), "{outcome:?}");
    let started = Instant::now();
    while engine.calls() == 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Only the manifest is reloaded: it names a new protocol version, which the campaign does not, and a much shorter grace.
    let mut renewed = identity.clone();
    renewed.limit_manifest_protocol_version = "limits.v2".to_owned();
    write_records(
        home,
        &manifest_json_with(
            &renewed,
            &ProjectionHook::ALL,
            &[("physical_drain_ms", 500)],
        ),
        &campaign_json(&identity),
    );
    let SliceOutcome::RotateMaintenance(handle) = owner.run_slice(&slice_budget()) else {
        panic!("a changed identity hands the supervisor back");
    };
    let started = Instant::now();
    let stopped = owner.stop_maintenance(&handle).await;
    let waited = started.elapsed();
    TestEngine::release(&held);
    let _ = owner.shutdown().await;
    assert!(stopped.is_err(), "{stopped:?}");
    assert!(
        waited >= Duration::from_secs(3),
        "the stop waited {waited:?}, the unapproved 500 ms rather than the approved 4 s"
    );
}

/// Stopping a supervisor after the admission records disappear waits the grace the last readable manifest approved, not the scheduler's idle delay.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_drain_after_the_records_vanish_keeps_the_approved_grace() {
    use support::embedding_fixtures::PROJECT;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("held-row", "held text");
    records(home);
    let engine = TestEngine::new();
    let held = engine.block_calls();
    let _release_on_unwind = support::embedding_fixtures::GateGuard(Arc::clone(&held));
    let scope = ProjectScope::new(PROJECT).unwrap();
    let owner = SearchLifecycleOwner::for_home(
        home,
        Arc::clone(&corpus.kernel),
        component(&engine, LocalEmbeddingsLimits::default()),
    )
    .with_roster(Arc::new(move || {
        vec![("project:a".to_owned(), scope.clone())]
    }));
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
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A reload approves a short drain grace, one slice reads it, then the records vanish.
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("physical_drain_ms", 500)],
        ),
        &campaign_json(&identity),
    );
    let outcome = owner.run_slice(&slice_budget());
    assert!(matches!(outcome, SliceOutcome::Current), "{outcome:?}");
    std::fs::remove_dir_all(home.join(ADMISSION_DIR)).unwrap();
    let SliceOutcome::RotateMaintenance(handle) = owner.run_slice(&slice_budget()) else {
        panic!("refused records hand the supervisor back");
    };
    let started = Instant::now();
    let stopped = owner.stop_maintenance(&handle).await;
    let waited = started.elapsed();
    TestEngine::release(&held);
    let _ = owner.shutdown().await;
    assert!(
        stopped.is_err(),
        "the held call outlives the grace: {stopped:?}"
    );
    assert!(
        waited < Duration::from_secs(2),
        "the drain waited {waited:?} against a 500 ms grace"
    );
}

/// Cancelling the loop while it drains a handed-back supervisor leaves that drain to the owner's shutdown, so the two together spend one grace.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_cancelled_loop_leaves_the_drain_to_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let engine = TestEngine::new();
    let held = engine.block_calls();
    let _release_on_unwind = support::embedding_fixtures::GateGuard(Arc::clone(&held));
    let roster = Arc::new(std::sync::Mutex::new(vec![(
        "project:a".to_owned(),
        ProjectScope::new(support::embedding_fixtures::PROJECT).unwrap(),
    )]));
    corpus.publish("held-row", "held text");
    records(home);
    let owner = {
        let roster = Arc::clone(&roster);
        Arc::new(
            SearchLifecycleOwner::for_home(
                home,
                Arc::clone(&corpus.kernel),
                component(&engine, LocalEmbeddingsLimits::default()),
            )
            .with_roster(Arc::new(move || roster.lock().unwrap().clone())),
        )
    };
    owner.set_drain_grace_for_test(Duration::from_millis(1_500));
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
    let started = Instant::now();
    while engine.calls() == 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // An emptied roster makes the next slice hand the supervisor back, and the loop begins draining it.
    roster.lock().unwrap().clear();
    let (draining, drain_started) = std::sync::mpsc::channel();
    owner.tap_slice_events_for_test(move |event| {
        if let SliceEvent::Draining(grace) = event {
            let _ = draining.send(*grace);
        }
    });
    let cancel = tokio_util::sync::CancellationToken::new();
    let loop_task = tokio::spawn(daemon::search_lifecycle_owner::run_slices(
        Arc::clone(&owner),
        cancel.clone(),
    ));
    let grace =
        tokio::task::spawn_blocking(move || drain_started.recv_timeout(Duration::from_secs(10)))
            .await
            .unwrap()
            .expect("the loop begins draining the handed-back supervisor");
    assert_eq!(grace, Duration::from_millis(1_500));
    let cancelled = Instant::now();
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(500), loop_task)
        .await
        .expect("a cancelled loop leaves the drain rather than finishing it")
        .unwrap();
    let shut = owner.shutdown().await;
    let waited = cancelled.elapsed();
    TestEngine::release(&held);
    let _ = owner.shutdown().await;
    assert!(shut.is_err(), "the held call outlives the grace: {shut:?}");
    assert!(
        waited < Duration::from_millis(2_200),
        "cancellation and shutdown spent {waited:?} against a 1.5 s grace"
    );
}

/// A request made after a reload changed the projection identity, before any slice rotated the selection, is judged under the new identity rather than denied on the old family's evidence.
#[test]
fn a_request_after_an_identity_change_is_judged_under_the_new_identity() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
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
    let ControlState::Current(current) = control(home) else {
        panic!("the first rebuild reached Current");
    };

    // The reload ships a new protocol version, which is part of the identity; the open family is foreign under it.
    let mut renewed = identity(&kernel_incarnation_id(home));
    renewed.limit_manifest_protocol_version = "limits.v2".to_owned();
    write_records(
        home,
        &manifest_json(&renewed, &ProjectionHook::ALL),
        &campaign_json(&renewed),
    );
    let mut request = rebuild(home);
    request.selected_generation = current.staged_seed_digest.clone().unwrap();
    request.consumer.consumer_id = "search-lifecycle-again".to_owned();
    request.attempt_id = "rebuild-under-limits-v2".to_owned();
    let outcome = owner.request(&request, now(), &slice_budget());
    assert!(outcome.is_ok(), "{outcome:?}");
}

/// A request that finds the manager held by a running slice waits only as long as its own budget allows.
#[test]
fn a_request_waiting_for_a_running_slice_ends_with_its_budget() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    // The slice pauses after preparing, holding the manager, until the test releases it.
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    owner.tap_slice_events_for_test(move |event| {
        if matches!(event, SliceEvent::Prepared { .. }) {
            let _ = reached_tx.send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
    });
    let outcome = std::thread::scope(|scope| {
        let slice = scope.spawn(|| owner.run_slice(&slice_budget()));
        reached
            .recv_timeout(Duration::from_secs(10))
            .expect("the slice reaches its pause");
        let started = Instant::now();
        let outcome = owner.request(&rebuild(home), now(), &budget(Duration::from_millis(200)));
        let waited = started.elapsed();
        release.send(()).unwrap();
        let _ = slice.join().unwrap();
        (outcome, waited)
    });
    assert!(
        matches!(outcome.0, Err(BuildError::Expired)),
        "{:?}",
        outcome.0
    );
    assert!(
        outcome.1 < Duration::from_secs(2),
        "the request waited {:?} on a 200 ms budget",
        outcome.1
    );
}

/// A slice that finds the manager held waits only as long as its own budget allows, so a cancelled slice returns without waiting for the holder.
#[test]
fn a_slice_waiting_for_the_manager_ends_with_its_budget() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    // A request pauses at its preflight tap while holding the manager, until the test releases it.
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    owner.tap_slice_events_for_test(move |event| {
        if matches!(event, SliceEvent::Prepared { .. }) {
            let _ = reached_tx.send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
    });
    let outcome = std::thread::scope(|scope| {
        let holder = scope.spawn(|| owner.run_slice(&slice_budget()));
        reached
            .recv_timeout(Duration::from_secs(10))
            .expect("the first slice reaches its pause");
        let short = budget(Duration::from_millis(200));
        short.cancel();
        let started = Instant::now();
        let outcome = owner.run_slice(&short);
        let waited = started.elapsed();
        release.send(()).unwrap();
        let _ = holder.join().unwrap();
        (outcome, waited)
    });
    assert!(
        matches!(outcome.0, SliceOutcome::Blocked(_)),
        "{:?}",
        outcome.0
    );
    assert!(
        outcome.1 < Duration::from_secs(2),
        "the cancelled slice waited {:?} for the manager",
        outcome.1
    );
}

/// A disable that finds the manager held waits only as long as its own budget allows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disable_waiting_for_the_manager_ends_with_its_budget() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = Arc::new(owner(home, &corpus.kernel));
    // A slice pauses at its preparation tap while holding the manager, until the test releases it.
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    owner.tap_slice_events_for_test(move |event| {
        if matches!(event, SliceEvent::Prepared { .. }) {
            let _ = reached_tx.send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
    });
    let holder = {
        let owner = Arc::clone(&owner);
        std::thread::spawn(move || owner.run_slice(&slice_budget()))
    };
    tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(10)))
        .await
        .unwrap()
        .expect("the slice reaches its pause");
    let started = Instant::now();
    let outcome = owner
        .disable(&budget(Duration::from_millis(200)), &mut |_| {})
        .await;
    let waited = started.elapsed();
    release.send(()).unwrap();
    let _ = holder.join().unwrap();
    assert!(matches!(outcome, Err(BuildError::Expired)), "{outcome:?}");
    assert!(
        waited < Duration::from_secs(2),
        "the disable waited {waited:?} on a 200 ms budget"
    );
}

/// A scheduled slice, whose budget carries no deadline of its own, waits for a held manager no longer than the manifest's slice bound.
#[test]
fn a_scheduled_slice_waits_for_the_manager_within_the_slice_bound() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("supervisor_slice_ms", 300)],
        ),
        &campaign_json(&identity),
    );
    let owner = owner(home, &corpus.kernel);
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    owner.tap_slice_events_for_test(move |event| {
        if matches!(event, SliceEvent::Prepared { .. }) {
            let _ = reached_tx.send(());
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
    });
    let outcome = std::thread::scope(|scope| {
        let holder = scope.spawn(|| owner.run_slice(&slice_budget()));
        reached
            .recv_timeout(Duration::from_secs(10))
            .expect("the first slice reaches its pause");
        let scheduled = EvalBudget::new(None, Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let started = Instant::now();
        let outcome = owner.run_slice(&scheduled);
        let waited = started.elapsed();
        release.send(()).unwrap();
        let _ = holder.join().unwrap();
        (outcome, waited)
    });
    assert!(
        matches!(outcome.0, SliceOutcome::Blocked(_)),
        "{:?}",
        outcome.0
    );
    assert!(
        outcome.1 < Duration::from_secs(2),
        "the scheduled slice waited {:?} against a 300 ms slice bound",
        outcome.1
    );
}

/// The slice bound runs from the slice's start: the deadline fixed before the wait for a held manager is the one the work runs under, and a reload during the wait cannot move it later.
#[test]
fn the_slice_bound_covers_the_wait_for_the_manager_and_the_work() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let identity = identity(&kernel_incarnation_id(home));
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("supervisor_slice_ms", 1_000)],
        ),
        &campaign_json(&identity),
    );
    let owner = owner(home, &corpus.kernel);
    // The first slice pauses at its preparation tap, holding the manager, until the second slice has fixed its deadline and the test releases it; the tap reports each slice's pre-lock and prepared deadlines.
    let (events_tx, events) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    let paused = std::sync::atomic::AtomicBool::new(false);
    owner.tap_slice_events_for_test(move |event| match event {
        SliceEvent::Waiting { deadline } => {
            let _ = events_tx.send(("waiting", *deadline));
        }
        SliceEvent::Prepared { deadline } => {
            let _ = events_tx.send(("prepared", *deadline));
            if !paused.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
            }
        }
        _ => {}
    });
    let next = |label: &str| loop {
        let (kind, deadline) = events
            .recv_timeout(Duration::from_secs(10))
            .expect("a slice event");
        if kind == label {
            return deadline.expect("a slice budget carries a deadline");
        }
    };
    std::thread::scope(|scope| {
        let holder = scope.spawn(|| owner.run_slice(&slice_budget()));
        next("waiting");
        next("prepared");
        let second = scope.spawn(|| owner.run_slice(&slice_budget()));
        let fixed = next("waiting");
        // A reload raising the bound while the second slice waits does not extend the deadline it fixed.
        write_records(
            home,
            &manifest_json_with(
                &identity,
                &ProjectionHook::ALL,
                &[("supervisor_slice_ms", 5_000)],
            ),
            &campaign_json(&identity),
        );
        std::thread::sleep(Duration::from_millis(300));
        release.send(()).unwrap();
        let prepared = next("prepared");
        let _ = holder.join().unwrap();
        let _ = second.join().unwrap();
        assert!(
            prepared <= fixed,
            "the prepared deadline moved {:?} past the one fixed before the wait",
            prepared.saturating_duration_since(fixed)
        );
    });
}

/// An operation whose budget is already over gets no manager even when it is free: a cancelled disable disables nothing.
#[tokio::test]
async fn an_expired_disable_does_not_take_a_free_manager() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());
    let expired = slice_budget();
    expired.cancel();
    let outcome = owner.disable(&expired, &mut |_| {}).await;
    assert!(matches!(outcome, Err(BuildError::Expired)), "{outcome:?}");
    assert!(
        !matches!(control(home), ControlState::Disabled(_)),
        "an expired disable persisted a stop"
    );
}

/// Disabling an owner with no family after its admission records are gone persists the stop and completes: there is no cleanup that would need the manifest's bounds.
#[tokio::test]
async fn a_disable_with_no_family_completes_without_admission_records() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());
    std::fs::remove_dir_all(home.join(ADMISSION_DIR)).unwrap();
    let outcome = owner.disable(&slice_budget(), &mut |_| {}).await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(matches!(control(home), ControlState::Disabled(_)));
}

/// A rebuild requested before any scheduled slice has run is judged on the records just read, not on the gate's initial closed state; a manifest no slice could prepare under is refused and records nothing.
#[test]
fn a_request_before_the_first_slice_is_judged_on_the_records() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let identity = identity(&kernel_incarnation_id(home));
    // Nine rows bound a replacement but not the coverage report a slice would read, so no slice could prepare under this manifest.
    write_records(
        home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("local_transaction_rows", 9)],
        ),
        &campaign_json(&identity),
    );
    let owner = owner(home, &corpus.kernel);
    let outcome = owner.request(&rebuild(home), now(), &slice_budget());
    assert!(
        matches!(outcome, Err(BuildError::Invalid(_))),
        "{outcome:?}"
    );
    assert!(matches!(control(home), ControlState::Absent));

    records(home);
    let outcome = owner.request(&rebuild(home), now(), &slice_budget());
    assert!(outcome.is_ok(), "{outcome:?}");
}

/// A request refused because the reloaded manifest cannot bound a slice closes admission on that manifest: the earlier grants are cancelled and readers are refused, while the lifecycle record is unchanged.
#[test]
fn a_request_refused_on_the_manifest_closes_admission() {
    // Nine rows or one row bound no coverage report; a zero slice bound runs no slice at all; a hundred thousand transaction bytes cannot bound the Current family's own replacement.
    for (name, value) in [
        ("local_transaction_rows", 9),
        ("local_transaction_rows", 1),
        ("supervisor_slice_ms", 0),
        ("local_transaction_bytes", 100_000),
    ] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let corpus = Corpus::open(home);
        corpus.seed();
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
        let grant = owner
            .admission()
            .gate()
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap();
        let ControlState::Current(current) = control(home) else {
            panic!("the rebuild reached Current");
        };

        let identity = identity(&kernel_incarnation_id(home));
        write_records(
            home,
            &manifest_json_with(&identity, &ProjectionHook::ALL, &[(name, value)]),
            &campaign_json(&identity),
        );
        let mut again = rebuild(home);
        again.selected_generation = current.staged_seed_digest.clone().unwrap();
        again.consumer.consumer_id = "search-lifecycle-again".to_owned();
        again.attempt_id = "rebuild-again".to_owned();
        let outcome = owner.request(&again, now(), &slice_budget());
        assert!(
            matches!(outcome, Err(BuildError::Invalid(_))),
            "{name}={value}: {outcome:?}"
        );
        assert!(
            grant.invalidated.is_cancelled(),
            "{name}={value}: a manifest no slice could prepare under cancels the earlier grants"
        );
        let pinned = owner.pin(&slice_budget());
        assert!(
            pinned.is_err(),
            "{name}={value}: a closed gate admits no reader: {:?}",
            pinned.err()
        );
        assert!(
            matches!(control(home), ControlState::Current(done) if done.attempt_id == current.attempt_id),
            "{name}={value}"
        );
    }
}

/// A request refused for its own sizing, with no change to the records, leaves the healthy family's admission as it is.
#[test]
fn a_request_refused_for_its_own_sizing_leaves_admission_open() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let identity = identity(&kernel_incarnation_id(home));
    // Three retry attempts afford one attempt per episode for an allowance of three, and none for four.
    write_records(
        home,
        &manifest_json_with(&identity, &ProjectionHook::ALL, &[("retry_attempts", 3)]),
        &campaign_json(&identity),
    );
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
    let grant = owner
        .admission()
        .gate()
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .unwrap();
    let ControlState::Current(current) = control(home) else {
        panic!("the rebuild reached Current");
    };

    let mut oversized = rebuild(home);
    oversized.allowance = 4;
    oversized.selected_generation = current.staged_seed_digest.clone().unwrap();
    oversized.consumer.consumer_id = "search-lifecycle-again".to_owned();
    oversized.attempt_id = "rebuild-again".to_owned();
    let outcome = owner.request(&oversized, now(), &slice_budget());
    assert!(
        matches!(outcome, Err(BuildError::Invalid(_))),
        "{outcome:?}"
    );
    assert!(
        !grant.invalidated.is_cancelled(),
        "a request's own sizing refusal cancels no grant"
    );
    assert!(owner.pin(&slice_budget()).is_ok());
    assert!(
        matches!(control(home), ControlState::Current(done) if done.attempt_id == current.attempt_id)
    );
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
    // The next slice observes the family's coverage again; the gate's denial does not hide it.
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

/// Disabling `EmbeddingBootstrap` blocks that hook without stopping the coverage observation the enabled hooks are judged on; restoring the manifest re-admits it in the same process.
#[test]
fn admission_recovers_in_process_after_the_gate_denies_the_selected_family() {
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

    let identity = identity(&kernel_incarnation_id(home));
    let without_bootstrap: Vec<ProjectionHook> = ProjectionHook::ALL
        .iter()
        .copied()
        .filter(|hook| *hook != ProjectionHook::EmbeddingBootstrap)
        .collect();
    write_records(
        home,
        &manifest_json(&identity, &without_bootstrap),
        &campaign_json(&identity),
    );
    let outcome = owner.run_slice(&slice_budget());
    assert!(matches!(outcome, SliceOutcome::Blocked(_)), "{outcome:?}");
    let gate = owner.admission().gate();
    assert_eq!(
        gate.admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::Disabled(ProjectionHook::EmbeddingBootstrap)
    );
    gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .expect("a hook that stays enabled is judged on the family's own coverage");

    records(home);
    let outcome = owner.run_slice(&slice_budget());
    assert!(matches!(outcome, SliceOutcome::Current), "{outcome:?}");
    gate.admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Dispatch)
        .expect("the restored manifest admits the family again");
    owner.pin(&slice_budget()).unwrap();
}

/// An active record past its deadline starts nothing. The slice still judges admission on the current observation, so it denies a selected family that trails the kernel instead of serving it on pre-deadline evidence.
#[test]
fn an_expired_active_record_is_still_judged_on_the_current_observation() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    corpus.publish("kept", "kept text");
    records(home);
    let owner = owner(home, &corpus.kernel);
    let _ = owner.run_slice(&slice_budget());

    let mut short = rebuild(home);
    short.deadline = now() + 5_000;
    owner.request(&short, now(), &slice_budget()).unwrap();
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(outcome, SliceOutcome::Advanced(RecoveryProgress::Selected)),
        "{outcome:?}"
    );
    let gate = owner.admission().gate();
    gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .expect("the selected family is fresh");

    // Inside the deadline margin no slice starts work; the kernel keeps moving.
    let until_margin = short.deadline - 1_000 - now();
    std::thread::sleep(Duration::from_millis(
        u64::try_from(until_margin).unwrap_or(0) + 50,
    ));
    for index in 0..=LAG_LIMIT {
        corpus.publish(&format!("later-{index}"), "later text");
    }
    let outcome = owner.run_slice(&slice_budget());
    assert!(
        matches!(&outcome, SliceOutcome::Blocked(reason) if reason.contains("deadline")),
        "{outcome:?}"
    );
    assert!(matches!(control(home), ControlState::Intent(_)));
    let denial = gate
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .expect_err("the expired slice observed the family against the moved kernel");
    assert!(matches!(denial, Denial::Stale { .. }), "{denial:?}");
    assert!(owner.pin(&slice_budget()).is_err());
}

/// A completed record whose family this owner already holds is reopened and revalidated once; an idle slice judges it on its coverage and kernel without taking the lifecycle transaction or writing under the lifecycle directory.
#[test]
fn an_idle_slice_on_a_held_current_family_reopens_nothing() {
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

    let lifecycle_dir = home.join("search-lifecycle");
    let changed_at = |path: &Path| {
        let metadata = std::fs::metadata(path).unwrap();
        (metadata.ctime(), metadata.ctime_nsec())
    };
    let before = changed_at(&lifecycle_dir);
    // Another lifecycle transaction holds the exclusive lock a reopen takes; an idle slice needs none of it.
    let transaction = LifecycleTransactionLock::acquire_exclusive(Some(home)).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let outcome = owner.run_slice(&slice_budget());
    assert!(matches!(outcome, SliceOutcome::Current), "{outcome:?}");
    assert_eq!(
        changed_at(&lifecycle_dir),
        before,
        "an idle slice writes nothing under the lifecycle directory"
    );
    drop(transaction);
    owner.pin(&slice_budget()).unwrap();
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
        (
            &[("supervisor_slice_ms", 0)][..],
            SpecRefusal::TooSmall("supervisor_slice_ms"),
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
