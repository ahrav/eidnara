//! The embedding maintenance supervisor against a real kernel, projection, and in-process Synapse host.
//! One budget bounds every stage of a pass and stops it at its deadline or cancellation; slices alternate so a sweep runs while backfill work is held; shutdown joins its slices, keeps native work owned until it exits, reports panics, and leaves durable unfinished work for the next incarnation.

mod support;

use std::future::Future;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use daemon::embedding_dispatch::{
    Blocked, DispatchBounds, DispatchEvent, DispatchFault, EmbeddingDispatcher, Stage,
};
use daemon::embedding_supervisor::{
    EmbeddingSupervisor, Maintained, SliceBounds, SliceKind, SliceOutcome, Stop, SupervisorEvent,
    Unresolved,
};
use daemon::search_projection::SearchProjection;
use host_runtime::synapse::{EmbeddingEngine, SynapseComponent, SynapseLimits};
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, ProjectScope};
use retrieval::dispatch::{Recovery, authorize_recovery};
use rusqlite::Connection;
use support::embedding_fixtures::{
    Corpus, DAY_MS, GateGuard, NOW, PROJECT, TestEngine, bounds, budget, component, eligibility,
    grant, inspect, lane, occurrence_of, search_path, tombstone,
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// The durable `(state, attempts, host_job_id)` of one occurrence's job, read outside every API under test.
fn row(data_home: &Path, occurrence: &str) -> (String, u32, Option<String>) {
    inspect(data_home)
        .query_row(
            "SELECT state,attempts,host_job_id FROM embedding_jobs WHERE occurrence_id=?1",
            [occurrence],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

fn stages(events: &[DispatchEvent]) -> Vec<(String, Stage, Instant)> {
    events
        .iter()
        .filter_map(|event| match event {
            DispatchEvent::Stage {
                job_id,
                stage,
                deadline,
            } => Some((job_id.clone(), *stage, *deadline)),
            _ => None,
        })
        .collect()
}

/// AC1: every stage of every job under one pass observes the budget's own deadline, and a budget already exhausted admits nothing. A renewed deadline would appear as a different instant in a stage event and fail the equality.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_budget_bounds_every_stage_of_every_job() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let objects: Vec<String> = ["a", "b"]
        .iter()
        .map(|name| corpus.publish(name, &format!("{name} text")))
        .collect();
    let (projection, rows) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();

    // An exhausted budget admits nothing and runs no inference.
    let spent = EvalBudget::new(
        Some(Instant::now() - Duration::from_millis(1)),
        Arc::new(AtomicBool::new(false)),
    );
    let mut events = Vec::new();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(eligibility(&project), &bounds(), &spent, NOW, &mut |e| {
                events.push(e)
            })
            .unwrap()
    });
    assert_eq!(end, Some(Blocked::BudgetExhausted));
    assert!(stages(&events).is_empty(), "{events:?}");
    assert_eq!(engine.calls(), 0);
    for object in &objects {
        assert_eq!(row(dir.path(), occurrence_of(&rows, object)).0, "pending");
    }

    // A live budget: every stage of both jobs carries the same deadline, in order.
    let live = budget(Duration::from_secs(5));
    let mut events = Vec::new();
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(eligibility(&project), &bounds(), &live, NOW, &mut |e| {
                events.push(e)
            })
            .unwrap()
    });
    assert_eq!(end, None);
    let observed = stages(&events);
    assert_eq!(observed.len(), 6);
    assert!(
        observed
            .iter()
            .all(|(_, _, deadline)| Some(*deadline) == live.deadline())
    );
    for object in &objects {
        let job = row(dir.path(), occurrence_of(&rows, object));
        let own: Vec<Stage> = observed
            .iter()
            .filter(|(id, _, _)| {
                inspect(dir.path())
                    .query_row(
                        "SELECT job_id FROM embedding_jobs WHERE occurrence_id=?1",
                        [occurrence_of(&rows, object)],
                        |r| r.get::<_, String>(0),
                    )
                    .unwrap()
                    == *id
            })
            .map(|(_, stage, _)| *stage)
            .collect();
        assert_eq!(own, vec![Stage::Admit, Stage::Poll, Stage::Publish]);
        assert_eq!(job.0, "embedded");
    }
}

/// AC1, AC2: a budget cancelled mid-poll stops the pass at once, before its deadline; the host keeps running the admitted call, the row stays admitted with its charge, and the next pass finishes it without a second inference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sticky_cancellation_stops_the_pass_and_keeps_native_work_owned() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("c", "c text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let gate = engine.block_calls();
    let _release = GateGuard(Arc::clone(&gate));
    let cancelled = budget(Duration::from_secs(30));
    let canceller = cancelled.clone();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let started = Instant::now();
    // The pass reports the poll stage once the row is admitted and charged, so the cancellation lands mid-poll rather than at a time the scheduler chooses.
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds(),
                &cancelled,
                NOW,
                &mut |event| {
                    if matches!(
                        event,
                        DispatchEvent::Stage {
                            stage: Stage::Poll,
                            ..
                        }
                    ) {
                        canceller.cancel();
                    }
                },
            )
            .unwrap()
    });
    assert_eq!(end, Some(Blocked::BudgetExhausted));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancellation, not the deadline, ended the pass"
    );
    let held = row(dir.path(), occurrence);
    assert_eq!((held.0.as_str(), held.1), ("admitted", 1));
    assert_eq!(
        synapse.job_status(held.2.as_deref().unwrap()),
        Some("running")
    );
    TestEngine::release(&gate);
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds(),
                &budget(Duration::from_secs(5)),
                NOW,
                &mut |_| {},
            )
            .unwrap()
    });
    assert_eq!(end, None);
    assert_eq!(row(dir.path(), occurrence).0, "embedded");
    assert_eq!(
        engine.calls(),
        1,
        "the held call was finished, not repeated"
    );
}

fn slice_bounds(slice: Duration) -> SliceBounds {
    SliceBounds {
        dispatch: bounds(),
        sweep_candidates: NonZeroUsize::new(16).unwrap(),
        slice,
        idle: Duration::from_millis(20),
    }
}

fn maintained(
    corpus: &Corpus,
    projection: Arc<SearchProjection>,
    synapse: Arc<SynapseComponent>,
) -> Maintained {
    Maintained {
        kernel: Arc::clone(&corpus.kernel),
        projection,
        synapse,
        project: ProjectScope::new(PROJECT).unwrap(),
        destination: ArtifactDestination::Remote,
    }
}

async fn next_event(events: &mut UnboundedReceiver<SupervisorEvent>) -> SupervisorEvent {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("an event within ten seconds")
        .expect("the supervisor is alive")
}

/// Bounds a whole wait rather than each receive, so a supervisor that keeps emitting events without reaching the expected one fails instead of hanging.
async fn within<T>(limit: Duration, wait: impl Future<Output = T>) -> T {
    tokio::time::timeout(limit, wait)
        .await
        .expect("the expected outcome within the limit")
}

/// Drains events up to and including the next `SliceEnded` of `kind` and returns its outcome; a `Stopped` event on the way is a test failure.
async fn ended(events: &mut UnboundedReceiver<SupervisorEvent>, kind: SliceKind) -> SliceOutcome {
    within(Duration::from_secs(30), async {
        loop {
            match next_event(events).await {
                SupervisorEvent::SliceEnded {
                    kind: ended,
                    outcome,
                } if ended == kind => return outcome,
                SupervisorEvent::Stopped(stop) => {
                    panic!("stopped before a {kind:?} slice ended: {stop:?}")
                }
                _ => {}
            }
        }
    })
    .await
}

/// AC2, AC3, AC4: slices alternate under their own budgets while native work is held; shutdown joins its slices but stays unresolved while the host still runs an admitted call, repeated requests neither duplicate work nor release anything, and once the call exits shutdown resolves with the row still admitted for the next incarnation, which finishes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn shutdown_joins_slices_and_stays_unresolved_while_native_work_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("held", "held text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let projection = Arc::new(projection);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let _release = GateGuard(Arc::clone(&gate));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::clone(&projection), Arc::clone(&synapse)),
        slice_bounds(Duration::from_secs(2)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // Backfill admits the job, waits for its result until the slice budget ends, and yields; a sweep slice then runs even though backfill work is still held.
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Backfill,
        deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    // The slice started no later than this receive, so its deadline is at most one slice from now.
    assert!(deadline <= Instant::now() + Duration::from_secs(2));
    match next_event(&mut events).await {
        SupervisorEvent::SliceEnded {
            kind: SliceKind::Backfill,
            outcome:
                SliceOutcome::Backfill {
                    end,
                    admitted,
                    published,
                    dispositions,
                },
        } => {
            assert_eq!(
                (end, admitted, published, dispositions),
                (Some(Blocked::BudgetExhausted), 1, 0, 0)
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted {
            kind: SliceKind::Sweep,
            ..
        }
    ));
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceEnded {
            kind: SliceKind::Sweep,
            ..
        }
    ));
    let held = row(dir.path(), &occurrence);
    assert_eq!((held.0.as_str(), held.1), ("admitted", 1));
    let host_job = held.2.clone().unwrap();
    assert_eq!(synapse.job_status(&host_job), Some("running"));

    // Shutdown joins the slice threads but the native call is still owned: unresolved, twice, with no second inference and nothing released.
    let first = supervisor.shutdown(Duration::from_secs(2)).await;
    assert_eq!(
        first,
        Err(Unresolved {
            slices: 0,
            native: 1
        })
    );
    let again = supervisor.shutdown(Duration::from_millis(100)).await;
    assert_eq!(
        again,
        Err(Unresolved {
            slices: 0,
            native: 1
        })
    );
    assert_eq!(engine.calls(), 1);
    assert_eq!(synapse.job_status(&host_job), Some("running"));
    assert_eq!(
        row(dir.path(), &occurrence),
        held,
        "cancellation preserves unfinished pending work"
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    let mut saw_stop = false;
    while let Ok(event) = events.try_recv() {
        if event == SupervisorEvent::Stopped(Stop::Shutdown) {
            saw_stop = true;
        }
    }
    assert!(saw_stop);

    // Native exit resolves the drain; the result is a held lease, not a published vector.
    TestEngine::release(&gate);
    let deadline = Instant::now() + Duration::from_secs(5);
    while synapse.job_status(&host_job) == Some("running") && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(engine.completed(), 1, "the native call exited");
    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    assert_eq!(report.held_results, 1);
    assert!(report.slices >= 2);
    assert_eq!(row(dir.path(), &occurrence).0, "admitted");

    // The next incarnation reconciles the admitted row and finishes it.
    drop(supervisor);
    let next = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let restarted = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::clone(&projection), next),
        slice_bounds(Duration::from_secs(5)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&restarted).run());
    within(Duration::from_secs(30), async {
        loop {
            match next_event(&mut events).await {
                SupervisorEvent::SliceEnded {
                    outcome:
                        SliceOutcome::Backfill {
                            published: 1,
                            admitted: 1,
                            dispositions: 0,
                            end: None,
                        },
                    ..
                } => break,
                SupervisorEvent::Stopped(stop) => panic!("{stop:?}"),
                _ => {}
            }
        }
    })
    .await;
    let done = row(dir.path(), &occurrence);
    assert_eq!((done.0.as_str(), done.1), ("embedded", 2));
    let report = restarted.shutdown(Duration::from_secs(5)).await.unwrap();
    assert_eq!(report.held_results, 0);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.calls(), 2);
}

/// A stopped row's running native call keeps shutdown unresolved until the call exits; its result then belongs to no lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_stopped_row_keeps_its_running_native_call_in_the_census() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("last", "last attempt");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let _release = GateGuard(Arc::clone(&gate));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    // The clock is advanced past the episode deadline once the row is admitted, so a later pass stops the row while its call runs.
    let clock = Arc::new(AtomicI64::new(NOW));
    let now = Arc::clone(&clock);
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), Arc::clone(&synapse)),
        SliceBounds {
            dispatch: DispatchBounds {
                grant: grant(1, NOW + DAY_MS),
                ..bounds()
            },
            ..slice_bounds(Duration::from_secs(2))
        },
        Arc::new(move || now.load(Ordering::SeqCst)),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // The first backfill admits the row and ends at its deadline with the call held.
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: Some(Blocked::BudgetExhausted),
            admitted: 1,
            published: 0,
            dispositions: 0,
        }
    );
    // Until the clock moves no backfill changes the admitted row, so this read is stable; the row is stopped by the first backfill that starts after the deadline passes, while a backfill that started earlier polls the held row once more.
    let held = row(dir.path(), &occurrence);
    assert_eq!((held.0.as_str(), held.1), ("admitted", 1));
    let host_job = held.2.clone().unwrap();
    clock.store(NOW + DAY_MS + 1, Ordering::SeqCst);
    within(Duration::from_secs(30), async {
        loop {
            match ended(&mut events, SliceKind::Backfill).await {
                SliceOutcome::Backfill {
                    end: None,
                    admitted: 0,
                    published: 0,
                    dispositions: 1,
                } => break,
                SliceOutcome::Backfill {
                    end: Some(Blocked::BudgetExhausted),
                    admitted: 0,
                    published: 0,
                    dispositions: 0,
                } => {}
                other => panic!("{other:?}"),
            }
        }
    })
    .await;
    let stopped = row(dir.path(), &occurrence);
    assert_eq!(
        (stopped.0.as_str(), stopped.1, stopped.2.as_deref()),
        ("failed", 1, None)
    );
    assert_eq!(synapse.job_status(&host_job), Some("running"));
    assert_eq!((engine.calls(), engine.completed()), (1, 0));

    // Shutdown joins the slices but the call this supervisor admitted is still running: nothing is resolved.
    assert_eq!(
        supervisor.shutdown(Duration::from_secs(2)).await,
        Err(Unresolved {
            slices: 0,
            native: 1
        })
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();

    // Once the call exits, shutdown resolves; the stopped row expects no result, so the supervisor reports no held lease.
    TestEngine::release(&gate);
    let deadline = Instant::now() + Duration::from_secs(5);
    while synapse.job_status(&host_job) == Some("running") && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(engine.completed(), 1);
    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    assert_eq!(report.held_results, 0);
    assert_eq!(row(dir.path(), &occurrence), stopped);
}

/// A blocked backfill takes the idle wait: a persistent blocker must not spin the loop at scheduler speed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocked_backfill_takes_the_idle_wait() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    // The lane's fingerprint differs from the projection's, so every backfill returns `BindingMismatch`.
    let mismatched = SynapseComponent::ready_with_engine(
        lane("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"),
        Arc::clone(&engine) as Arc<dyn EmbeddingEngine>,
        SynapseLimits::default(),
    )
    .unwrap();
    let idle = Duration::from_millis(300);
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), Arc::new(mismatched)),
        SliceBounds {
            idle,
            ..slice_bounds(Duration::from_secs(5))
        },
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // Each `SliceStarted` deadline is the producer's own start plus the slice length, so the gap between two deadlines is the gap between two starts and does not depend on when this task receives them.
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Backfill,
        deadline: backfill_deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: Some(Blocked::BindingMismatch),
            admitted: 0,
            published: 0,
            dispositions: 0,
        }
    );
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Sweep,
        deadline: sweep_deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    assert!(
        sweep_deadline >= backfill_deadline + idle,
        "a blocked backfill did no work, so the next slice waits the idle interval"
    );

    supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.calls(), 0);
}

/// A backfill that only recorded dispositions made progress: the next slice starts at once rather than after the idle wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_backfill_of_dispositions_alone_does_not_idle() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    corpus.publish("b", "b text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let idle = Duration::from_secs(5);
    let (sender, mut events) = unbounded_channel();
    // An expired grant stops every pending row before admission; one row per pass leaves a backlog behind the first.
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        SliceBounds {
            dispatch: DispatchBounds {
                max_jobs: NonZeroUsize::new(1).unwrap(),
                grant: grant(3, NOW - 1),
                ..bounds()
            },
            idle,
            ..slice_bounds(Duration::from_secs(5))
        },
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: None,
            admitted: 0,
            published: 0,
            dispositions: 1,
        }
    );
    let progressed_at = Instant::now();
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted {
            kind: SliceKind::Sweep,
            ..
        }
    ));
    assert!(
        progressed_at.elapsed() < idle,
        "a pass that recorded a disposition made progress and does not wait"
    );

    supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.calls(), 0);
}

/// A read that fails before anything is decided is reported and retried after the idle wait; it does not stop the supervisor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_eligibility_read_is_retried_not_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("a", "a text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        slice_bounds(Duration::from_secs(5)),
        Arc::new(|| NOW),
        sender,
    );
    supervisor.inject_dispatch_fault_for_test(DispatchFault::RefuseEligibilityRead);
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    let SliceOutcome::ReadFailed(message) = ended(&mut events, SliceKind::Backfill).await else {
        panic!("the failed read is the slice's outcome")
    };
    assert!(message.contains("database is locked"), "{message}");

    // The next backfill runs the pass the failed read postponed.
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: None,
            admitted: 1,
            published: 1,
            dispositions: 0,
        }
    );
    assert_eq!(row(dir.path(), &occurrence).0, "embedded");

    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
}

/// AC3: a slice that panics is reported with its payload, stops the supervisor, and is joined by shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_slice_is_reported_and_stops_the_supervisor() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        slice_bounds(Duration::from_secs(5)),
        Arc::new(|| NOW),
        sender,
    );
    supervisor.panic_next_slice_for_test();
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted { .. }
    ));
    assert_eq!(
        next_event(&mut events).await,
        SupervisorEvent::Stopped(Stop::Panicked(
            "maintenance slice panicked for the test".to_owned()
        ))
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();

    // The stop is terminal: a later `run` on the same supervisor, with no shutdown in between, returns without scheduling a slice.
    tokio::time::timeout(Duration::from_secs(2), Arc::clone(&supervisor).run())
        .await
        .expect("a stopped supervisor's run returns at once");
    assert!(
        events.try_recv().is_err(),
        "no slice started and no second stop was reported"
    );

    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(
        report.stop,
        Some(Stop::Panicked(
            "maintenance slice panicked for the test".to_owned()
        ))
    );
    assert_eq!(report.slices, 1);
    assert_eq!(engine.calls(), 0, "the panicking slice admitted nothing");
}

/// AC3: a slice held inside a projection write outlives the grace: shutdown reports the unjoined slice, releases nothing, and a later request joins it once the store is free.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn grace_expiry_reports_an_unjoined_slice_and_a_later_request_joins_it() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    // Another writer holds the projection file, so the slice's first write waits on the store's busy timeout.
    let blocker = Connection::open(search_path(dir.path())).unwrap();
    blocker.busy_timeout(Duration::ZERO).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        slice_bounds(Duration::from_secs(10)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted {
            kind: SliceKind::Backfill,
            ..
        }
    ));
    // Give the slice time to reach the held write before asking it to stop; the store's busy wait is what keeps it from joining.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let unresolved = supervisor.shutdown(Duration::from_millis(100)).await;
    assert_eq!(
        unresolved,
        Err(Unresolved {
            slices: 1,
            native: 0
        })
    );
    assert_eq!(
        engine.calls(),
        0,
        "nothing was admitted while the store was held"
    );
    blocker.execute_batch("COMMIT").unwrap();
    let report = supervisor.shutdown(Duration::from_secs(10)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    assert_eq!(report.slices, 1);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        engine.calls(),
        0,
        "the cancelled slice admitted nothing once the store was free"
    );
}

/// A budget that expires while lane binding waits on the store stops the pass before the eligibility read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_budget_spent_during_binding_stops_before_the_eligibility_read() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    // Another writer holds the projection file past the budget's deadline, so the binding write returns from its busy wait only after the budget ended.
    let blocker = Connection::open(search_path(dir.path())).unwrap();
    blocker.busy_timeout(Duration::ZERO).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        blocker.execute_batch("COMMIT").unwrap();
    });
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    // The injected read failure distinguishes budget exhaustion before the eligibility read from a read that ran.
    dispatcher.inject_fault_for_test(DispatchFault::RefuseEligibilityRead);
    let spent_during_bind = budget(Duration::from_millis(200));
    let end = tokio::task::block_in_place(|| {
        dispatcher.run_pass(
            eligibility(&project),
            &bounds(),
            &spent_during_bind,
            NOW,
            &mut |_| {},
        )
    });
    releaser.join().unwrap();
    assert!(
        matches!(end, Ok(Some(Blocked::BudgetExhausted))),
        "the pass ended with its budget, without opening the eligibility read: {end:?}"
    );
    assert_eq!(engine.calls(), 0);
}

/// A zero-length slice that makes no progress waits `idle` before the next slice to prevent a scheduler-speed loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exhausted_slice_without_progress_takes_the_idle_wait() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let idle = Duration::from_millis(300);
    let (sender, mut events) = unbounded_channel();
    // A zero slice is exhausted the moment it starts, so every backfill and every sweep ends with the budget and nothing done.
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        SliceBounds {
            idle,
            ..slice_bounds(Duration::ZERO)
        },
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // Each `SliceStarted` deadline is the producer's own start plus the slice length, so the gap between two deadlines is the gap between two starts and does not depend on when this task receives them.
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Backfill,
        deadline: backfill_deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: Some(Blocked::BudgetExhausted),
            admitted: 0,
            published: 0,
            dispositions: 0,
        }
    );
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Sweep,
        deadline: sweep_deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    assert!(
        sweep_deadline >= backfill_deadline + idle,
        "an exhausted slice that moved nothing waits the idle interval before the next slice"
    );

    supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.calls(), 0);
}

/// Shutdown before the spawned loop is first polled and a repeated shutdown each report the same final stop.
#[tokio::test]
async fn shutdown_before_the_loop_is_polled_reports_a_final_stop() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        slice_bounds(Duration::from_secs(5)),
        Arc::new(|| NOW),
        sender,
    );
    // On a current-thread runtime the spawned loop cannot run until this task yields, and draining an empty tracker never yields, so shutdown resolves before the loop takes its token.
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    assert_eq!(report.slices, 0);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        supervisor.shutdown(Duration::from_secs(2)).await.unwrap(),
        report,
        "a repeated shutdown reports what the first one did"
    );
    assert_eq!(
        next_event(&mut events).await,
        SupervisorEvent::Stopped(Stop::Shutdown)
    );
    assert!(events.try_recv().is_err(), "the stop is reported once");
    assert_eq!(engine.calls(), 0);
}

/// A reauthorized row resubmits under a new episode while its earlier host job runs, so one durable job owns two host jobs; shutdown stays unresolved until each has exited.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_reauthorized_row_keeps_its_earlier_native_call_in_the_census() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("again", "again text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let projection = Arc::new(projection);
    let engine = TestEngine::new();
    let first_gate = engine.block_calls();
    let _release_first = GateGuard(Arc::clone(&first_gate));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    // The clock is advanced past the first episode's deadline once the row is admitted, so a later pass stops the row while its call runs.
    let clock = Arc::new(AtomicI64::new(NOW));
    let now = Arc::clone(&clock);
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::clone(&projection), Arc::clone(&synapse)),
        SliceBounds {
            dispatch: DispatchBounds {
                grant: grant(1, NOW + DAY_MS),
                ..bounds()
            },
            ..slice_bounds(Duration::from_secs(2))
        },
        Arc::new(move || now.load(Ordering::SeqCst)),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // The first backfill admits the row and reaches its deadline while the gate holds the embedding call.
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: Some(Blocked::BudgetExhausted),
            admitted: 1,
            published: 0,
            dispositions: 0,
        }
    );
    // Until the clock moves no backfill changes the admitted row; the first backfill that starts after the deadline passes stops the row while the first embedding call runs.
    let first_job = row(dir.path(), &occurrence).2.unwrap();
    let expired = NOW + DAY_MS + 1;
    clock.store(expired, Ordering::SeqCst);
    within(Duration::from_secs(30), async {
        loop {
            match ended(&mut events, SliceKind::Backfill).await {
                SliceOutcome::Backfill {
                    end: None,
                    admitted: 0,
                    published: 0,
                    dispositions: 1,
                } => break,
                SliceOutcome::Backfill {
                    end: Some(Blocked::BudgetExhausted),
                    admitted: 0,
                    published: 0,
                    dispositions: 0,
                } => {}
                other => panic!("{other:?}"),
            }
        }
    })
    .await;
    assert_eq!(row(dir.path(), &occurrence).0, "failed");
    assert_eq!(synapse.job_status(&first_job), Some("running"));

    // The next admitting backfill submits a second host job before the first host call returns.
    let second_gate = engine.block_calls();
    let _release_second = GateGuard(Arc::clone(&second_gate));
    let job_id: String = inspect(dir.path())
        .query_row(
            "SELECT job_id FROM embedding_jobs WHERE occurrence_id=?1",
            [&occurrence],
            |r| r.get(0),
        )
        .unwrap();
    let recovery = projection
        .write(|conn| {
            authorize_recovery(conn, &job_id, "auth-1", grant(1, expired + DAY_MS), expired)
        })
        .unwrap();
    assert!(matches!(recovery, Recovery::Granted { .. }), "{recovery:?}");
    within(Duration::from_secs(30), async {
        loop {
            match ended(&mut events, SliceKind::Backfill).await {
                SliceOutcome::Backfill { admitted: 1, .. } => break,
                SliceOutcome::Backfill {
                    admitted: 0,
                    published: 0,
                    dispositions: 0,
                    ..
                } => {}
                other => panic!("{other:?}"),
            }
        }
    })
    .await;
    let reopened = row(dir.path(), &occurrence);
    assert_eq!((reopened.0.as_str(), reopened.1), ("admitted", 1));
    let second_job = reopened.2.unwrap();
    assert_ne!(first_job, second_job, "a new episode is a new host job");
    assert_eq!(synapse.job_status(&first_job), Some("running"));
    // The single CPU permit serializes the two calls, so the second call is queued until the first returns.
    assert!(matches!(
        synapse.job_status(&second_job),
        Some("queued" | "running")
    ));
    assert_eq!(engine.calls(), 1);

    assert_eq!(
        supervisor.shutdown(Duration::from_secs(2)).await,
        Err(Unresolved {
            slices: 0,
            native: 2
        }),
        "the census counts the first host job and the second host job"
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();

    TestEngine::release(&first_gate);
    let deadline = Instant::now() + Duration::from_secs(5);
    while synapse.job_status(&second_job) != Some("running") && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(synapse.job_status(&first_job), Some("ready"));
    assert_eq!(
        supervisor.shutdown(Duration::from_secs(2)).await,
        Err(Unresolved {
            slices: 0,
            native: 1
        }),
        "the second host job runs after the first returned"
    );

    TestEngine::release(&second_gate);
    let deadline = Instant::now() + Duration::from_secs(5);
    while synapse.job_status(&second_job) == Some("running") && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(engine.completed(), 2);
    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    assert_eq!(
        report.held_results, 1,
        "the stopped episode expects no result; the reopened episode holds one"
    );
    assert_eq!(row(dir.path(), &occurrence).0, "admitted");
}

/// Identities the host still holds at the head of the sweep order take one sweep each; the cursor carries across slices, so a free identity behind them is reclaimed instead of never being selected.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_held_head_of_the_sweep_order_does_not_starve_identities_behind_it() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    for name in ["a", "b", "c", "d"] {
        corpus.publish(name, &format!("{name} text"));
    }
    let (projection, _) = corpus.bootstrap(dir.path());
    let projection = Arc::new(projection);
    let ordered: Vec<String> = {
        let conn = inspect(dir.path());
        let mut statement = conn
            .prepare("SELECT occurrence_id FROM embedding_jobs ORDER BY job_id")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap()
    };
    let (held, free) = ordered.split_at(3);
    // The free identity dies before dispatch, so no host ever holds it.
    tombstone(&projection, &free[0], 50);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let _release = GateGuard(Arc::clone(&gate));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::clone(&projection), synapse),
        SliceBounds {
            dispatch: DispatchBounds {
                result_wait: Duration::from_millis(50),
                ..bounds()
            },
            sweep_candidates: NonZeroUsize::new(1).unwrap(),
            ..slice_bounds(Duration::from_secs(2))
        },
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // The first backfill admits the three identities ahead of the free one into gated calls; retiring them afterwards leaves the host holding their jobs.
    assert!(matches!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill { admitted: 3, .. }
    ));
    for occurrence in held {
        tombstone(&projection, occurrence, 60);
    }

    // One candidate per sweep: the free identity is fourth in order, so its reclamation needs sweeps that resume where the last one ended.
    let reclaimed = within(Duration::from_secs(30), async {
        let mut sweeps = 0;
        loop {
            let SliceOutcome::Sweep(report) = ended(&mut events, SliceKind::Sweep).await else {
                panic!()
            };
            sweeps += 1;
            if report.jobs_reclaimed > 0 || sweeps == 8 {
                break report.jobs_reclaimed;
            }
        }
    })
    .await;
    assert_eq!(
        reclaimed, 1,
        "the free identity behind three held ones is reclaimed within a pass over the table"
    );

    supervisor
        .shutdown(Duration::from_secs(2))
        .await
        .expect_err("the gated calls are still owned");
    TestEngine::release(&gate);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
}

/// A stopped row's host job leaves the census once its call has exited: the host holds a result no row expects, so the entry is retired during maintenance rather than kept until shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_settled_call_no_row_expects_leaves_the_census_during_maintenance() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("settled", "settled text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let _release = GateGuard(Arc::clone(&gate));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let clock = Arc::new(AtomicI64::new(NOW));
    let now = Arc::clone(&clock);
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), Arc::clone(&synapse)),
        SliceBounds {
            dispatch: DispatchBounds {
                grant: grant(1, NOW + DAY_MS),
                ..bounds()
            },
            ..slice_bounds(Duration::from_secs(2))
        },
        Arc::new(move || now.load(Ordering::SeqCst)),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    assert!(matches!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill { admitted: 1, .. }
    ));
    let host_job = row(dir.path(), &occurrence).2.unwrap();
    assert_eq!(supervisor.tracked_host_jobs_for_test(), 1);
    // The episode deadline passes, so the next backfill to start stops the row while the host still runs the call.
    clock.store(NOW + DAY_MS + 1, Ordering::SeqCst);
    within(Duration::from_secs(30), async {
        loop {
            if let SliceOutcome::Backfill {
                dispositions: 1, ..
            } = ended(&mut events, SliceKind::Backfill).await
            {
                break;
            }
        }
    })
    .await;
    assert_eq!(row(dir.path(), &occurrence).0, "failed");
    assert_eq!(synapse.job_status(&host_job), Some("running"));
    assert_eq!(
        supervisor.tracked_host_jobs_for_test(),
        1,
        "a stopped row's running call stays tracked"
    );

    // The call exits into a retained result no row expects; the next backfill retires the entry.
    TestEngine::release(&gate);
    within(Duration::from_secs(30), async {
        loop {
            ended(&mut events, SliceKind::Backfill).await;
            if synapse.job_status(&host_job) == Some("ready")
                && supervisor.tracked_host_jobs_for_test() == 0
            {
                break;
            }
        }
    })
    .await;

    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.held_results, 0);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
}

/// Shutdown gives an admitted native call the rest of its grace to exit instead of reporting it unresolved the moment the slices have joined.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn shutdown_waits_the_grace_for_an_admitted_call_to_exit() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("brief", "brief text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let _release = GateGuard(Arc::clone(&gate));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), Arc::clone(&synapse)),
        slice_bounds(Duration::from_secs(2)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert!(matches!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill { admitted: 1, .. }
    ));
    let host_job = row(dir.path(), &occurrence).2.unwrap();
    assert_eq!(synapse.job_status(&host_job), Some("running"));

    // The call exits well inside the grace, after the slices have joined.
    let releasing = Arc::clone(&gate);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        TestEngine::release(&releasing);
    });
    let asked = Instant::now();
    let report = supervisor
        .shutdown(Duration::from_secs(5))
        .await
        .expect("a call that exits within the grace resolves shutdown");
    assert!(asked.elapsed() < Duration::from_secs(5));
    assert_eq!(synapse.job_status(&host_job), Some("ready"));
    assert_eq!(
        report.held_results, 1,
        "the admitted row still expects the result the host now holds"
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
}

/// A submission whose charge rolled back leaves the row pending with no reference to the host job, so the job's result is nothing the census reports as held.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_submission_whose_charge_rolled_back_claims_no_result() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("uncharged", "uncharged text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let _release = GateGuard(Arc::clone(&gate));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    // A long idle keeps the next backfill, which would re-admit the row under the same host job, from running before shutdown.
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), Arc::clone(&synapse)),
        SliceBounds {
            idle: Duration::from_secs(10),
            ..slice_bounds(Duration::from_secs(2))
        },
        Arc::new(|| NOW),
        sender,
    );
    supervisor.inject_dispatch_fault_for_test(DispatchFault::RefuseChargeStatement);
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: None,
            admitted: 0,
            published: 0,
            dispositions: 0,
        }
    );
    let uncharged = row(dir.path(), &occurrence);
    assert_eq!(
        (uncharged.0.as_str(), uncharged.1, uncharged.2),
        ("pending", 0, None)
    );
    assert_eq!(engine.calls(), 1, "the host runs the submitted call");
    assert_eq!(supervisor.tracked_host_jobs_for_test(), 1);

    TestEngine::release(&gate);
    let settled = Instant::now();
    while engine.completed() == 0 && settled.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let report = supervisor.shutdown(Duration::from_secs(5)).await.unwrap();
    assert_eq!(
        report.held_results, 0,
        "no admitted row expects the uncharged submission's result"
    );
    assert_eq!(supervisor.tracked_host_jobs_for_test(), 0);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
}

/// A census taken while a slice is still inside its charge write must not lose the claim that the charge is about to make: the result is ready but unclaimed at that instant, and once the charge commits the admitted row expects it.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_census_during_a_blocked_charge_keeps_the_result_the_charge_then_claims() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("charged late", "charged late text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), Arc::clone(&synapse)),
        slice_bounds(Duration::from_secs(2)),
        Arc::new(|| NOW),
        sender,
    );
    // On `Submitted`, another writer takes the projection file, so the charge that follows waits on the busy timeout while the ungated inference completes.
    let blocker: Arc<std::sync::Mutex<Option<Connection>>> = Arc::new(std::sync::Mutex::new(None));
    let submitted: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let search = search_path(dir.path());
    let (hold, seen) = (Arc::clone(&blocker), Arc::clone(&submitted));
    supervisor.tap_dispatch_events_for_test(move |event| {
        if let DispatchEvent::Submitted { host_job_id, .. } = event {
            let conn = Connection::open(&search).unwrap();
            conn.busy_timeout(Duration::ZERO).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            *hold.lock().unwrap() = Some(conn);
            *seen.lock().unwrap() = Some(host_job_id.clone());
        }
    });
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Backfill,
        deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    let host_job = within(Duration::from_secs(10), async {
        loop {
            if let Some(host_job) = submitted.lock().unwrap().clone() {
                break host_job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    within(Duration::from_secs(10), async {
        while synapse.job_status(&host_job) != Some("ready") {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    // The grace ends with the slice still inside its charge: the result is ready and no row has claimed it yet.
    assert_eq!(
        supervisor.shutdown(Duration::from_millis(200)).await,
        Err(Unresolved {
            slices: 1,
            native: 0
        })
    );

    // The charge commits after the slice's deadline, so the guard refuses the publication and the admitted row keeps the ready result as a held lease.
    tokio::time::sleep_until((deadline + Duration::from_millis(100)).into()).await;
    blocker
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .execute_batch("COMMIT")
        .unwrap();
    let report = supervisor.shutdown(Duration::from_secs(10)).await.unwrap();
    let settled = row(dir.path(), &occurrence);
    assert_eq!(
        (settled.0.as_str(), settled.2.as_deref()),
        ("admitted", Some(host_job.as_str()))
    );
    assert_eq!(
        report.held_results, 1,
        "the row admitted by the late charge expects the ready result"
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
}

/// One supervisor runs one loop: a second `run` on another clone returns at once while the first keeps scheduling, so two loops never interleave slices over the same census.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_second_run_returns_while_the_first_loop_owns_the_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        slice_bounds(Duration::from_secs(5)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted {
            kind: SliceKind::Backfill,
            ..
        }
    ));
    tokio::time::timeout(Duration::from_secs(2), Arc::clone(&supervisor).run())
        .await
        .expect("a second run returns while the first loop runs");
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: None,
            admitted: 1,
            published: 1,
            dispositions: 0,
        },
        "the first loop's slice is the only one that ran"
    );
    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.calls(), 1, "one loop ran one inference");
}
