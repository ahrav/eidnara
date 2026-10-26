use super::source_capture_tests::delete_session;
use super::use_outcome_writer_tests::{
    bound, seed_memories, served, transform_with_usage_leaving_serves_pending,
};
use super::*;
use crate::use_outcome_writer::LifecycleWrite;

fn lifecycle_counts(handler: &Handler) -> Vec<(&'static str, u64)> {
    LifecycleWrite::ALL
        .iter()
        .map(|write| (write.name(), handler.use_outcome_lifecycle.get(*write)))
        .filter(|(_, count)| *count > 0)
        .collect()
}

fn lifecycle(records: &[kernel::UseOutcomeRecord]) -> Vec<(Option<i64>, Option<i64>)> {
    records
        .iter()
        .map(|record| (record.serve.deleted_at, record.serve.last_transform_at))
        .collect()
}

/// A handler with one recorded serve of `mem-a` for session `ses`, from one HARD fold; the fold's committed pass also touched the record.
async fn served_handler() -> (Handler, Arc<MemoryStore>, tempfile::TempDir, PathBuf) {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, dir, project) =
        handler_with_store_and_kernel(producer, default_test_config()).await;
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a.")],
    );
    let first = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(first["action"], "HARD", "{first}");
    assert_eq!(served(&handler, &project).len(), 1);
    (handler, store, dir, project)
}

/// `session.delete` marks the session's records deleted in its Kernel step, before the memory-store delete: a stop between the stores leaves the records marked and the memory rows intact, and the retry answers `ok`, deletes the rows, keeps the first timestamp, and commits nothing new. The served set reads the same records and decision rows after `ok`.
#[tokio::test(flavor = "current_thread")]
async fn a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time() {
    let (handler, store, _dir, project) = served_handler().await;
    let before = served(&handler, &project);
    handler
        .source_capture
        .stop_after_retirement
        .store(true, std::sync::atomic::Ordering::Release);
    let stopped = delete_session(&handler, "ses").await;
    assert_eq!(stopped["error"], "injected_stop", "{stopped}");
    let marked = served(&handler, &project);
    let deleted_at = marked[0]
        .serve
        .deleted_at
        .expect("the Kernel step committed");
    assert!(
        store.load("ses").unwrap().row_version.is_some(),
        "the memory rows are intact"
    );
    let kernel = handler.kernel.kernel_store().unwrap();
    let tip = kernel.tip().unwrap();

    let retried = delete_session(&handler, "ses").await;
    assert_eq!(retried["ok"], true, "{retried}");
    assert!(retried["deleted_rows"].as_u64().unwrap() > 0);
    assert_eq!(kernel.tip().unwrap(), tip, "the retry finds no live record");
    let after = served(&handler, &project);
    assert_eq!(after[0].serve.deleted_at, Some(deleted_at));
    assert_eq!(after.len(), before.len());
    assert_eq!(
        after[0].serve.use_outcome_id,
        before[0].serve.use_outcome_id
    );
    assert_eq!(after[0].decision_object_ids, before[0].decision_object_ids);
    assert_eq!(
        lifecycle_counts(&handler),
        [
            ("deleted_marked", 1),
            ("delete_nothing_live", 1),
            ("transform_touched", 1)
        ]
    );
}

/// A session with no record deletes with no Kernel commit for its records. A Kernel that is not Ready answers `kernel_unavailable`, counted, with the memory rows intact and `deleted_at` null; once the Kernel is Ready, one repeat answers `ok` and marks the records.
#[tokio::test(flavor = "current_thread")]
async fn a_delete_with_nothing_live_commits_nothing_and_an_unready_kernel_converges_on_retry() {
    let producer = Arc::new(ProducerState::default());
    let (empty, _store, _dir, _project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    let _ = call_transform(&empty, vec![ck("m1", 1, "hello")]).await;
    let tip = empty.kernel.kernel_store().unwrap().tip().unwrap();
    let deleted = delete_session(&empty, "ses").await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert_eq!(empty.kernel.kernel_store().unwrap().tip().unwrap(), tip);
    assert_eq!(lifecycle_counts(&empty), [("delete_nothing_live", 1)]);

    let (handler, store, dir, project) = served_handler().await;
    let before = served(&handler, &project);
    handler
        .kernel
        .mark_unavailable(kernel_routes::UnavailableKind::Store);
    let refused = delete_session(&handler, "ses").await;
    assert_eq!(refused["error"], "kernel_unavailable", "{refused}");
    assert!(store.load("ses").unwrap().row_version.is_some());
    handler
        .kernel
        .open(
            dir.path().join("kernel"),
            StoreOpenPolicy::default(),
            handler.cancel.clone(),
        )
        .await;
    assert_eq!(handler.kernel.state(), kernel_routes::KernelState::Ready);
    assert_eq!(served(&handler, &project)[0].serve.deleted_at, None);
    let retried = delete_session(&handler, "ses").await;
    assert_eq!(retried["ok"], true, "{retried}");
    let after = served(&handler, &project);
    assert!(after[0].serve.deleted_at.is_some());
    assert_eq!(
        after[0].serve.use_outcome_id,
        before[0].serve.use_outcome_id
    );
    assert_eq!(after[0].decision_object_ids, before[0].decision_object_ids);
    assert_eq!(
        lifecycle_counts(&handler),
        [
            ("deleted_marked", 1),
            ("delete_kernel_unavailable", 1),
            ("transform_touched", 1)
        ]
    );
}

/// A delete step whose commit ran but whose answer was an error leaves the memory rows intact and the records marked: a transient error answers `kernel_unavailable` and the repeat answers `ok` with nothing live and the first timestamp kept, while a conflict answers `source_retirement_failed`.
#[tokio::test(flavor = "current_thread")]
async fn a_delete_step_with_an_unknown_outcome_converges_on_retry() {
    for (fault, code) in [
        (kernel::KernelError::Io, "kernel_unavailable"),
        (kernel::KernelError::Conflict, "source_retirement_failed"),
    ] {
        let (handler, store, _dir, project) = served_handler().await;
        *handler.use_outcome_delete_fault.lock().unwrap() = Some(fault);
        let failed = delete_session(&handler, "ses").await;
        assert_eq!(failed["error"], code, "{failed}");
        assert!(store.load("ses").unwrap().row_version.is_some(), "{code}");
        let deleted_at = served(&handler, &project)[0].serve.deleted_at;
        assert!(deleted_at.is_some(), "{code}: the commit ran");
        let tip = handler.kernel.kernel_store().unwrap().tip().unwrap();
        let retried = delete_session(&handler, "ses").await;
        assert_eq!(retried["ok"], true, "{code}: {retried}");
        assert_eq!(served(&handler, &project)[0].serve.deleted_at, deleted_at);
        assert_eq!(handler.kernel.kernel_store().unwrap().tip().unwrap(), tip);
        assert_eq!(
            handler
                .use_outcome_lifecycle
                .get(LifecycleWrite::DeleteNothingLive),
            1,
            "{code}"
        );
        assert_eq!(
            handler
                .use_outcome_lifecycle
                .get(LifecycleWrite::DeleteKernelUnavailable),
            u64::from(code == "kernel_unavailable"),
            "{code}"
        );
    }
}

/// The route stays bound after `session.delete`, so the session can keep passing; a later delete marks the records its later folds wrote, because each delete commits under its own attempt key. A later fold over a composition the deleted record already holds replays that record.
#[tokio::test(flavor = "current_thread")]
async fn a_second_delete_marks_records_written_after_the_first() {
    let (handler, _store, _dir, project) = served_handler().await;
    let first = delete_session(&handler, "ses").await;
    assert_eq!(first["ok"], true, "{first}");
    let first_mark = served(&handler, &project)[0].serve.deleted_at;
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a."), ("mem-b", "Rule b.")],
    );
    let continued = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(continued["action"], "HARD", "{continued}");
    let records = served(&handler, &project);
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].serve.deleted_at, None);

    let second = delete_session(&handler, "ses").await;
    assert_eq!(second["ok"], true, "{second}");
    let records = served(&handler, &project);
    assert_eq!(records[0].serve.deleted_at, first_mark);
    assert!(records[1].serve.deleted_at.is_some());
    assert_eq!(
        handler
            .use_outcome_lifecycle
            .get(LifecycleWrite::DeletedMarked),
        2
    );
}

/// Route teardown without `session.delete` writes nothing: `deleted_at` stays null and the memory rows stay.
#[tokio::test(flavor = "current_thread")]
async fn a_route_torn_down_after_a_hard_fold_leaves_its_records_live() {
    let (handler, store, _dir, project) = served_handler().await;
    let tip = handler.kernel.kernel_store().unwrap().tip().unwrap();
    handler.unbind_route(test_route(7));
    assert!(
        matches!(
            handler.resolve_binding(test_route(7), "ses"),
            Err(BindingError::Unbound)
        ),
        "the route was torn down"
    );
    assert_eq!(served(&handler, &project)[0].serve.deleted_at, None);
    assert!(store.load("ses").unwrap().row_version.is_some());
    assert_eq!(handler.kernel.kernel_store().unwrap().tip().unwrap(), tip);
    assert_eq!(lifecycle_counts(&handler), [("transform_touched", 1)]);
}

/// Across a committing pass, a non-committing pass, a pass whose clock reads before the stored value, and a pass with the Kernel unavailable, `last_transform_at` never moves backward and moves only on a committing pass; every pass answers `ok` and no update appends a commit.
#[tokio::test(flavor = "current_thread")]
async fn last_transform_at_moves_forward_only_on_committing_passes() {
    let (handler, _store, _dir, project) = served_handler().await;
    let kernel = handler.kernel.kernel_store().unwrap();
    let binding = bound(&project);
    let read = || {
        lifecycle(
            &kernel
                .served_set(binding.kernel_project.digest(), &binding.harness, "ses")
                .unwrap()
                .unwrap(),
        )
    };
    // The HARD fold's settle ran after its record was written, so the fold's own pass touched it.
    let [(None, Some(first))] = read()[..] else {
        panic!("{:?}", read());
    };
    let tip = kernel.tip().unwrap();

    let repeat = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(repeat["committed"], false, "{repeat}");
    assert_eq!(read(), [(None, Some(first))]);

    tokio::time::sleep(Duration::from_millis(2)).await;
    let later = call_transform(&handler, vec![ck("m1", 1, "hello"), ck("m2", 2, "more")]).await;
    assert_eq!(later["committed"], true, "{later}");
    let [(None, Some(second))] = read()[..] else {
        panic!("{:?}", read());
    };
    assert!(second > first);
    assert_eq!(kernel.tip().unwrap(), tip, "the update appends no commit");

    // A stored value ahead of this host's clock stands for a pass whose clock reads earlier.
    let ahead = second + 3_600_000;
    let budget = kernel::applicability::EvalBudget::new(
        Some(Instant::now() + Duration::from_secs(5)),
        Arc::default(),
    );
    kernel
        .advance_use_outcome_last_transform(
            &budget,
            binding.kernel_project.digest(),
            &binding.harness,
            "ses",
            ahead,
        )
        .unwrap();
    let behind = call_transform(
        &handler,
        vec![
            ck("m1", 1, "hello"),
            ck("m2", 2, "more"),
            ck("m3", 3, "again"),
        ],
    )
    .await;
    assert_eq!(behind["committed"], true, "{behind}");
    assert_eq!(read(), [(None, Some(ahead))]);

    handler
        .kernel
        .mark_unavailable(kernel_routes::UnavailableKind::Store);
    let unavailable = call_transform(
        &handler,
        vec![
            ck("m1", 1, "hello"),
            ck("m2", 2, "more"),
            ck("m3", 3, "again"),
            ck("m4", 4, "once more"),
        ],
    )
    .await;
    assert_eq!(unavailable["status"], "ok", "{unavailable}");
    assert_eq!(unavailable["committed"], true, "{unavailable}");
    assert_eq!(read(), [(None, Some(ahead))]);
    assert_eq!(
        lifecycle_counts(&handler),
        [
            ("transform_touched", 2),
            ("transform_skipped_unavailable", 1)
        ]
    );
}

/// A committing pass that meets a held Kernel writer skips the update within the write budget and answers `ok`; the next committing pass catches up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_writer_skips_the_update_and_the_next_committing_pass_catches_up() {
    let (handler, _store, _dir, project) = served_handler().await;
    let kernel = handler.kernel.kernel_store().unwrap();
    let touched = served(&handler, &project)[0].serve.last_transform_at;
    let (entered, held) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = {
        let kernel = Arc::clone(&kernel);
        std::thread::spawn(move || {
            kernel
                .commit(kernel_route_fixtures::intent("hold-writer"), |_| {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    Ok(String::new())
                })
                .unwrap();
        })
    };
    held.recv().unwrap();
    let skipped = call_transform(&handler, vec![ck("m1", 1, "hello"), ck("m2", 2, "more")]).await;
    release.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(skipped["status"], "ok", "{skipped}");
    assert_eq!(skipped["committed"], true, "{skipped}");
    assert_eq!(
        served(&handler, &project)[0].serve.last_transform_at,
        touched
    );

    let caught_up = call_transform(
        &handler,
        vec![
            ck("m1", 1, "hello"),
            ck("m2", 2, "more"),
            ck("m3", 3, "again"),
        ],
    )
    .await;
    assert_eq!(caught_up["committed"], true, "{caught_up}");
    assert!(served(&handler, &project)[0].serve.last_transform_at > touched);
    assert_eq!(
        lifecycle_counts(&handler),
        [("transform_touched", 2), ("transform_skipped_deadline", 1)]
    );
}

/// A pass's Kernel writes share one wait: once an earlier write spent it, a later touch answers `Deadline` with a free writer and leaves the record as it was.
#[tokio::test(flavor = "current_thread")]
async fn a_spent_pass_budget_skips_the_touch() {
    let (handler, _store, _dir, project) = served_handler().await;
    let kernel = handler.kernel.kernel_store().unwrap();
    let binding = bound(&project);
    let touched = served(&handler, &project)[0]
        .serve
        .last_transform_at
        .unwrap();
    let budget = crate::use_outcome_writer::PassWriteBudget::default();
    budget.spend(|_| std::thread::sleep(Duration::from_millis(260)));
    let write = budget.spend(|budget| {
        crate::use_outcome_writer::touch_session(
            &kernel,
            budget,
            &binding.kernel_project,
            &binding.harness,
            "ses",
            touched + 1,
        )
    });
    assert_eq!(write, Some(LifecycleWrite::TransformSkippedDeadline));
    assert_eq!(
        served(&handler, &project)[0].serve.last_transform_at,
        Some(touched)
    );
}

/// A HARD pass whose serve write counts `deadline` leaves its touch a full Kernel wait: the serve runs as a tracked job with its own waits, so with the writer free the touch advances `last_transform_at`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_serve_deadline_leaves_the_touch_its_own_kernel_wait() {
    let (handler, _store, _dir, project) = served_handler().await;
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a."), ("mem-b", "Rule b.")],
    );
    let touched = served(&handler, &project)[0]
        .serve
        .last_transform_at
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let occupied = handler.use_outcome_writes.occupy_every_slot();
    let refold =
        transform_with_usage_leaving_serves_pending(&handler, big_messages(), 48_000, 50_000).await;
    drop(occupied);
    assert_eq!(refold["action"], "HARD", "{refold}");
    assert_eq!(
        handler
            .use_outcome_writes
            .get(crate::use_outcome_writer::UseOutcomeWrite::Deadline),
        1,
        "the serve found no write slot"
    );
    assert!(
        served(&handler, &project)[0]
            .serve
            .last_transform_at
            .unwrap()
            > touched,
        "the touch advanced the first fold's record"
    );
    assert_eq!(lifecycle_counts(&handler), [("transform_touched", 2)]);
}

/// A touch that waits on a held Kernel writer runs beside the rest of the settle: the pass's completion lands in the memory store's pass trace while the touch waits, the holder then releases the writer, and the touch advances `last_transform_at`. The thread scope joins the touch before the response returns. The response build and its trace write must fit in what remains of the pass's 250 ms Kernel wait, or the touch counts `transform_skipped_deadline`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_touch_waiting_on_the_writer_overlaps_the_settle_and_lands_before_the_response() {
    let (handler, store, _dir, project) = served_handler().await;
    let kernel = handler.kernel.kernel_store().unwrap();
    let touched = served(&handler, &project)[0]
        .serve
        .last_transform_at
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let completed = |store: &MemoryStore| {
        store
            .load_pass_trace("ses")
            .unwrap()
            .unwrap()
            .last_completed_at_ms
    };
    let before = completed(&store);
    let (entered, held) = std::sync::mpsc::channel();
    let holder = {
        let kernel = Arc::clone(&kernel);
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            kernel
                .commit(kernel_route_fixtures::intent("hold-writer"), |_| {
                    entered.send(()).unwrap();
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while completed(&store) == before {
                        assert!(
                            Instant::now() < deadline,
                            "the pass completed only after its touch"
                        );
                        std::thread::sleep(Duration::from_micros(200));
                    }
                    Ok(String::new())
                })
                .unwrap();
        })
    };
    held.recv().unwrap();
    let next = call_transform(&handler, vec![ck("m1", 1, "hello"), ck("m2", 2, "more")]).await;
    holder.join().unwrap();
    assert_eq!(next["committed"], true, "{next}");
    let counts = lifecycle_counts(&handler);
    assert!(
        served(&handler, &project)[0]
            .serve
            .last_transform_at
            .unwrap()
            > touched,
        "the touch did not land beside the settle: {counts:?}"
    );
    assert_eq!(counts, [("transform_touched", 2)]);
}

/// A session id the Kernel's secret detector matches still deletes: the delete receipt key carries a digest of the id, so the identity check passes, a session with nothing live commits nothing, and a session with a live record is marked.
#[tokio::test(flavor = "current_thread")]
async fn a_session_id_that_reads_as_a_secret_still_deletes() {
    let (handler, _store, _dir, project) = served_handler().await;
    let kernel = handler.kernel.kernel_store().unwrap();
    let binding = bound(&project);
    // Assembled at run time so the repository's secret scanners see no credential-shaped literal.
    let session_id = format!("{}={}", "token", "abcdefghijklmnopqrstuvwxyz0123456789");
    let marked = crate::use_outcome_writer::mark_session_deleted(
        &kernel,
        &binding.kernel_project,
        &binding.harness,
        &session_id,
        1,
    );
    assert_eq!(marked, Ok(LifecycleWrite::DeleteNothingLive));
    assert_eq!(
        crate::use_outcome_writer::mark_session_deleted(
            &kernel,
            &binding.kernel_project,
            &binding.harness,
            "ses",
            1,
        ),
        Ok(LifecycleWrite::DeletedMarked),
        "a plain id still selects and marks its live record"
    );
}
