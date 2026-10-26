use super::source_capture_tests::delete_session;
use super::use_outcome_writer_tests::{bound, seed_memories, served};
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

/// A session with no record deletes with no Kernel commit for its records, and a Kernel that is not Ready answers `kernel_unavailable` with the memory rows intact and `deleted_at` null.
#[tokio::test(flavor = "current_thread")]
async fn a_delete_with_nothing_live_or_no_ready_kernel_marks_nothing() {
    let producer = Arc::new(ProducerState::default());
    let (empty, _store, _dir, _project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    let _ = call_transform(&empty, vec![ck("m1", 1, "hello")]).await;
    let tip = empty.kernel.kernel_store().unwrap().tip().unwrap();
    let deleted = delete_session(&empty, "ses").await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert_eq!(empty.kernel.kernel_store().unwrap().tip().unwrap(), tip);
    assert_eq!(lifecycle_counts(&empty), [("delete_nothing_live", 1)]);

    let (handler, store, _dir, project) = served_handler().await;
    let kernel = handler.kernel.kernel_store().unwrap();
    handler
        .kernel
        .mark_unavailable(kernel_routes::UnavailableKind::Store);
    let refused = delete_session(&handler, "ses").await;
    assert_eq!(refused["error"], "kernel_unavailable", "{refused}");
    assert!(store.load("ses").unwrap().row_version.is_some());
    let binding = bound(&project);
    let records = kernel
        .served_set(binding.kernel_project.digest(), &binding.harness, "ses")
        .unwrap()
        .unwrap();
    assert_eq!(records[0].serve.deleted_at, None);
}

/// Route teardown without `session.delete` writes nothing: `deleted_at` stays null and the memory rows stay.
#[tokio::test(flavor = "current_thread")]
async fn a_route_torn_down_after_a_hard_fold_leaves_its_records_live() {
    let (handler, store, _dir, project) = served_handler().await;
    let tip = handler.kernel.kernel_store().unwrap().tip().unwrap();
    handler.unbind_route(test_route(7));
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

    let later = call_transform(&handler, vec![ck("m1", 1, "hello"), ck("m2", 2, "more")]).await;
    assert_eq!(later["committed"], true, "{later}");
    let [(None, Some(second))] = read()[..] else {
        panic!("{:?}", read());
    };
    assert!(second >= first);
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
    tokio::time::sleep(Duration::from_millis(2)).await;
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
