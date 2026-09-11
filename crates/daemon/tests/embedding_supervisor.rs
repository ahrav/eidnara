//! The embedding maintenance supervisor against a real kernel, projection, and in-process Synapse host.
//! One budget bounds every stage of a pass and stops it at its deadline or cancellation; slices alternate so a sweep runs while backfill work is held; shutdown joins its slices, keeps native work owned until it exits, reports panics, and leaves durable unfinished work for the next incarnation.

mod support;

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use daemon::embedding_dispatch::{
    Blocked, DispatchBounds, DispatchEvent, EmbeddingDispatcher, Stage,
};
use daemon::embedding_supervisor::{
    EmbeddingSupervisor, Maintained, SliceBounds, SliceKind, SliceOutcome, Stop, SupervisorEvent,
    Unresolved,
};
use daemon::search_projection::SearchProjection;
use host_runtime::synapse::{EmbeddingEngine, SynapseComponent, SynapseLimits};
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, ProjectScope};
use rusqlite::Connection;
use support::embedding_fixtures::{
    Corpus, DAY_MS, GateGuard, NOW, PROJECT, TestEngine, bounds, budget, component, eligibility,
    grant, inspect, lane, occurrence_of, search_path,
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
    let cancelled = budget(Duration::from_secs(30));
    let canceller = cancelled.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        canceller.cancel();
    });
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let started = Instant::now();
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds(),
                &cancelled,
                NOW,
                &mut |_| {},
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

/// Drains events up to and including the next `SliceEnded` of `kind` and returns its outcome; a `Stopped` event on the way is a test failure.
async fn ended(events: &mut UnboundedReceiver<SupervisorEvent>, kind: SliceKind) -> SliceOutcome {
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
    let started_at = Instant::now();
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Backfill,
        deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    assert!(deadline <= started_at + Duration::from_secs(2) + Duration::from_millis(100));
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
    // The episode deadline passes; a backfill that started before the clock moved polls the held row once more, and the first backfill after it stops the row before polling while the host still runs the call.
    clock.store(NOW + DAY_MS + 1, Ordering::SeqCst);
    let held = row(dir.path(), &occurrence);
    assert_eq!((held.0.as_str(), held.1), ("admitted", 1));
    let host_job = held.2.clone().unwrap();
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

    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Backfill {
            end: Some(Blocked::BindingMismatch),
            admitted: 0,
            published: 0,
            dispositions: 0,
        }
    );
    let blocked_at = Instant::now();
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted {
            kind: SliceKind::Sweep,
            ..
        }
    ));
    assert!(
        blocked_at.elapsed() >= idle,
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
    supervisor.fail_next_read_for_test();
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    let SliceOutcome::ReadFailed(message) = ended(&mut events, SliceKind::Backfill).await else {
        panic!("the failed read is the slice's outcome")
    };
    assert!(message.contains("database is locked"), "{message}");
    assert_eq!(engine.calls(), 0);

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
