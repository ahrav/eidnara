#![cfg(feature = "test-support")]

mod source_fixture;

use kernel::applicability::EvalBudget;
use kernel::{
    CommitPageBounds, CommitReadError, CommitReadRequest, ExportWindow, KernelError, KernelStore,
    SourceExportError, SourceHold, SourceHoldCheckPhase, SourceHoldError, SourcePageBounds,
};
use source_fixture::*;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn timed_budget() -> EvalBudget {
    EvalBudget::new(
        Some(Instant::now() + Duration::from_millis(40)),
        Arc::new(AtomicBool::new(false)),
    )
}

fn page_bounds() -> SourcePageBounds {
    SourcePageBounds {
        max_rows: NonZeroUsize::new(100).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_row_bytes: NonZeroU64::new(1 << 20).unwrap(),
    }
}

fn commit_bounds() -> CommitPageBounds {
    CommitPageBounds {
        max_commits: NonZeroUsize::new(100).unwrap(),
        max_rows: NonZeroUsize::new(1000).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
    }
}

fn assert_all_refuse(
    store: &KernelStore,
    hold: &SourceHold,
    request: &CommitReadRequest,
    budget: impl Fn() -> EvalBudget,
) {
    let binding = &hold.binding;
    let id = &hold.hold_id;
    let deadline = SourceHoldError::Kernel(KernelError::Deadline);
    assert_eq!(
        store.tip_within_budget(&budget()),
        Err(KernelError::Deadline)
    );
    assert_eq!(
        store.database_incarnation_id_within_budget(&budget()),
        Err(KernelError::Deadline)
    );
    assert_eq!(
        store.capture_commit_read_target_within_budget(&budget()),
        Err(KernelError::Deadline)
    );
    assert_eq!(
        store.read_complete_commits_within_budget(&budget(), request, commit_bounds()),
        Err(CommitReadError::Kernel(KernelError::Deadline))
    );
    assert_eq!(
        store.outbox_consumer_checkpoint_within_budget(&budget(), CONSUMER),
        Err(KernelError::Deadline)
    );
    assert_eq!(
        store.capture_source_hold_within_budget(&budget(), binding, wide()),
        Err(deadline)
    );
    assert_eq!(
        store.extend_source_hold_within_budget(
            &budget(),
            binding,
            id,
            request.through_commit,
            wide_admission()
        ),
        Err(deadline)
    );
    assert_eq!(
        store.source_hold_status_within_budget(&budget(), binding, id, wall_ms()),
        Err(deadline)
    );
    assert_eq!(
        store.acknowledge_through_source_hold_if_within_budget(
            &budget(),
            binding,
            id,
            hold.snapshot,
            wall_ms(),
            || -> Option<()> { panic!("authorization must not run after refused acquisition") }
        ),
        Err(deadline)
    );
    assert_eq!(
        store.acknowledge_through_source_hold_within_budget(
            &budget(),
            binding,
            id,
            hold.snapshot,
            wall_ms()
        ),
        Err(deadline)
    );
    assert_eq!(
        store.release_source_hold_within_budget(&budget(), binding, id, wall_ms()),
        Err(deadline)
    );
    assert_eq!(
        store.reconcile_source_holds_within_budget(&budget(), CONSUMER, wall_ms()),
        Err(deadline)
    );
    assert_eq!(
        store.export_source_page_within_budget(
            &budget(),
            binding,
            id,
            wall_ms(),
            ExportWindow::Snapshot,
            None,
            page_bounds()
        ),
        Err(SourceExportError::Kernel(KernelError::Deadline))
    );
}

#[test]
fn all_source_apis_refuse_cancelled_free_guards_and_held_guards_without_progress() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "first", 1, "first text");
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    fixture.publish("messages", "second", 1, "second text");
    let target = fixture.store.capture_commit_read_target().unwrap();
    let request = CommitReadRequest {
        consumer_id: CONSUMER.into(),
        incarnation: target.incarnation,
        after_commit: 0,
        through_commit: target.through_commit,
    };
    let materialized = fixture.store.materialized_outbox_rows_for_test();
    let object_reads = fixture.store.verified_object_reads_for_test();
    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_all_refuse(&fixture.store, &hold, &request, || cancelled.clone());
    assert_all_refuse(&fixture.store, &hold, &request, || {
        EvalBudget::new(Some(Instant::now()), Arc::new(AtomicBool::new(false)))
    });
    let (done, wait) = mpsc::channel();
    std::thread::scope(|scope| {
        let mut observed = None;
        let refused = fixture.store.commit(intent("hold-writer"), |_| {
            fixture.store.with_readers_held_for_test(|| {
                scope.spawn(|| {
                    assert_all_refuse(&fixture.store, &hold, &request, timed_budget);
                    done.send(()).unwrap();
                });
                observed = Some(wait.recv_timeout(Duration::from_secs(3)));
            });
            Err(KernelError::Fault)
        });
        assert_eq!(refused, Err(KernelError::Fault));
        observed
            .unwrap()
            .expect("all refusals arrive while every kernel guard is still held");
    });
    assert_eq!(
        fixture.count("SELECT count(*) FROM capture_pins WHERE pin_kind='source_hold'"),
        1
    );
    assert_eq!(fixture.pin_refs(&hold.hold_id).len(), hold.references);
    assert_eq!(fixture.checkpoint(), 0);
    assert_eq!(
        fixture.store.materialized_outbox_rows_for_test(),
        materialized
    );
    assert_eq!(fixture.store.verified_object_reads_for_test(), object_reads);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&hold.binding, &hold.hold_id, wall_ms())
            .unwrap(),
        hold
    );
    assert!(
        fixture
            .store
            .read_complete_commits(&request, commit_bounds())
            .is_ok()
    );

    let fresh = || {
        EvalBudget::new(
            Some(Instant::now() + Duration::from_secs(5)),
            Arc::new(AtomicBool::new(false)),
        )
    };
    let store = &fixture.store;
    let binding = &hold.binding;
    let id = &hold.hold_id;
    assert_eq!(
        store.tip_within_budget(&fresh()).unwrap(),
        target.through_commit
    );
    assert_eq!(
        store
            .database_incarnation_id_within_budget(&fresh())
            .unwrap()
            .len(),
        32
    );
    assert_eq!(
        store
            .capture_commit_read_target_within_budget(&fresh())
            .unwrap(),
        target
    );
    let page = store
        .read_complete_commits_within_budget(&fresh(), &request, commit_bounds())
        .unwrap();
    assert_eq!(page.through, request.through_commit);
    assert!(!page.commits.is_empty());
    assert_eq!(
        store
            .outbox_consumer_checkpoint_within_budget(&fresh(), CONSUMER)
            .unwrap(),
        Some(0)
    );
    let captured = store
        .capture_source_hold_within_budget(&fresh(), binding, wide())
        .unwrap();
    assert_eq!(captured.snapshot, request.through_commit);
    assert_eq!(captured.references, 2);
    assert_eq!(
        store
            .source_hold_status_within_budget(&fresh(), binding, id, wall_ms())
            .unwrap(),
        hold
    );
    let page = store
        .export_source_page_within_budget(
            &fresh(),
            binding,
            id,
            wall_ms(),
            ExportWindow::Snapshot,
            None,
            page_bounds(),
        )
        .unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].text.as_deref(), Some("first text"));
    let extended = store
        .extend_source_hold_within_budget(
            &fresh(),
            binding,
            id,
            request.through_commit,
            wide_admission(),
        )
        .unwrap();
    assert_eq!(extended.references, 2);
    let mut authorized = false;
    assert!(
        store
            .acknowledge_through_source_hold_if_within_budget(
                &fresh(),
                binding,
                id,
                hold.snapshot,
                wall_ms(),
                || {
                    authorized = true;
                    Some(())
                }
            )
            .unwrap()
    );
    assert!(authorized);
    store
        .acknowledge_through_source_hold_within_budget(
            &fresh(),
            binding,
            id,
            hold.snapshot,
            wall_ms(),
        )
        .unwrap();
    assert_eq!(fixture.checkpoint(), hold.snapshot);
    assert!(
        store
            .reconcile_source_holds_within_budget(&fresh(), CONSUMER, wall_ms())
            .unwrap()
            .is_empty()
    );
    store
        .release_source_hold_within_budget(&fresh(), binding, id, wall_ms())
        .unwrap();
    assert_eq!(
        store.source_hold_status(binding, id, wall_ms()),
        Err(SourceHoldError::Invalid(
            kernel::SourceHoldInvalidity::Released
        ))
    );
}

#[test]
fn sqlite_writer_contention_uses_original_deadline_not_connection_busy_timeout() {
    let fixture = Fixture::open();
    let mut connection =
        rusqlite::Connection::open(fixture.root.path().join("kernel.sqlite")).unwrap();
    let held = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let (done, wait) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let result = fixture.store.capture_source_hold_within_budget(
                &timed_budget(),
                &fixture.binding(),
                wide(),
            );
            done.send(result).unwrap();
        });
        let result = wait.recv_timeout(Duration::from_secs(1));
        drop(held);
        assert_eq!(
            result.expect("deadline while external SQLite writer is held"),
            Err(SourceHoldError::Kernel(KernelError::Deadline))
        );
    });
    assert_eq!(fixture.count("SELECT count(*) FROM capture_pins"), 0);
    assert!(
        fixture
            .store
            .capture_source_hold(&fixture.binding(), wide())
            .is_ok()
    );
}

#[test]
fn cancellation_at_capture_admission_and_ack_authorization_leaves_no_mutation() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "first", 1, "first text");
    let budget = EvalBudget::unbounded();
    let mut admitted = false;
    let result = fixture.store.capture_source_hold_with_hook_for_test(
        &budget,
        &fixture.binding(),
        wide(),
        |_| {
            admitted = true;
            budget.cancel();
        },
    );
    assert!(admitted);
    assert_eq!(result, Err(SourceHoldError::Kernel(KernelError::Deadline)));
    assert_eq!(fixture.count("SELECT count(*) FROM capture_pins"), 0);
    assert_eq!(fixture.count("SELECT count(*) FROM capture_pin_refs"), 0);
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let budget = EvalBudget::unbounded();
    let mut authorized = false;
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold_if_within_budget(
                &budget,
                &hold.binding,
                &hold.hold_id,
                hold.snapshot,
                wall_ms(),
                || {
                    authorized = true;
                    budget.cancel();
                    Some(())
                }
            ),
        Err(SourceHoldError::Kernel(KernelError::Deadline))
    );
    assert!(authorized);
    assert_eq!(fixture.checkpoint(), 0);
}

#[test]
fn status_cancellation_stops_before_next_artifact_and_releases_readers() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "first", 1, "first text");
    fixture.publish("messages", "second", 1, "second text");
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let budget = EvalBudget::unbounded();
    let before = fixture.store.verified_object_reads_for_test();
    let mut boundaries = 0;
    let result = fixture.store.source_hold_status_with_hook_for_test(
        &budget,
        &hold.binding,
        &hold.hold_id,
        wall_ms(),
        |phase| {
            fixture.store.with_readers_held_for_test(|| ());
            if phase == SourceHoldCheckPhase::BeforeObjectRead {
                boundaries += 1;
                if boundaries == 2 {
                    budget.cancel();
                }
            }
        },
    );
    assert_eq!(boundaries, 2);
    assert_eq!(fixture.store.verified_object_reads_for_test() - before, 1);
    assert_eq!(result, Err(SourceHoldError::Kernel(KernelError::Deadline)));
    assert!(
        fixture
            .store
            .source_hold_status(&hold.binding, &hold.hold_id, wall_ms())
            .is_ok()
    );
}

#[test]
fn export_cancellation_after_snapshot_returns_no_cursor_or_artifact_and_releases_readers() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "first", 1, "first text");
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let budget = EvalBudget::unbounded();
    let cancel = budget.clone();
    let store = Arc::clone(&fixture.store);
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    let reads = fixture.store.verified_object_reads_for_test();
    let result = KernelStore::with_source_export_after_snapshot_hook_for_test(
        move || {
            store.with_readers_held_for_test(|| ());
            hook_fired.store(true, Ordering::SeqCst);
            cancel.cancel();
        },
        || {
            fixture.store.export_source_page_within_budget(
                &budget,
                &hold.binding,
                &hold.hold_id,
                wall_ms(),
                ExportWindow::Snapshot,
                None,
                page_bounds(),
            )
        },
    );
    assert!(fired.load(Ordering::SeqCst));
    assert_eq!(
        result,
        Err(SourceExportError::Kernel(KernelError::Deadline))
    );
    assert_eq!(fixture.store.verified_object_reads_for_test(), reads);
    let page = fixture
        .store
        .export_source_page(
            &hold.binding,
            &hold.hold_id,
            wall_ms(),
            ExportWindow::Snapshot,
            None,
            page_bounds(),
        )
        .unwrap();
    assert_eq!(page.rows[0].text.as_deref(), Some("first text"));
}

#[test]
fn bounded_capture_export_complete_commits_and_ack_preserve_fencing() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "first", 1, "first text");
    let budget = EvalBudget::unbounded();
    let hold = fixture
        .store
        .capture_source_hold_within_budget(&budget, &fixture.binding(), wide())
        .unwrap();
    fixture.publish("messages", "second", 1, "second text");
    let target = fixture
        .store
        .capture_commit_read_target_within_budget(&budget)
        .unwrap();
    let request = CommitReadRequest {
        consumer_id: CONSUMER.into(),
        incarnation: target.incarnation,
        after_commit: hold.snapshot,
        through_commit: target.through_commit,
    };
    assert_eq!(
        fixture
            .store
            .read_complete_commits_within_budget(&budget, &request, commit_bounds())
            .unwrap(),
        fixture
            .store
            .read_complete_commits(&request, commit_bounds())
            .unwrap()
    );
    assert!(
        fixture
            .store
            .acknowledge_through_source_hold_within_budget(
                &budget,
                &hold.binding,
                &hold.hold_id,
                target.through_commit,
                wall_ms()
            )
            .is_err()
    );
    fixture
        .store
        .extend_source_hold_within_budget(
            &budget,
            &hold.binding,
            &hold.hold_id,
            target.through_commit,
            wide_admission(),
        )
        .unwrap();
    let page = fixture
        .store
        .export_source_page_within_budget(
            &budget,
            &hold.binding,
            &hold.hold_id,
            wall_ms(),
            ExportWindow::CatchUp {
                through: target.through_commit,
            },
            None,
            page_bounds(),
        )
        .unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].text.as_deref(), Some("second text"));
    fixture
        .store
        .source_hold_status_within_budget(&budget, &hold.binding, &hold.hold_id, wall_ms())
        .unwrap();
    struct CancelOnDrop(EvalBudget);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold_if_within_budget(
                &budget,
                &hold.binding,
                &hold.hold_id,
                target.through_commit,
                wall_ms(),
                || Some(CancelOnDrop(budget.clone()))
            ),
        Ok(true)
    );
    assert!(budget.is_exhausted());
    let budget = EvalBudget::unbounded();
    assert_eq!(
        fixture
            .store
            .outbox_consumer_checkpoint_within_budget(&budget, CONSUMER)
            .unwrap(),
        Some(target.through_commit)
    );
    fixture.store.invalidate_writer_fence_for_test().unwrap();
    assert_eq!(
        fixture.store.release_source_hold_within_budget(
            &budget,
            &hold.binding,
            &hold.hold_id,
            wall_ms()
        ),
        Err(SourceHoldError::Kernel(KernelError::FenceLost))
    );
}

#[test]
fn status_and_export_reacquire_readers_with_the_original_budget() {
    for status in [true, false] {
        let mut fixture = Fixture::open();
        fixture.publish("messages", "first", 1, "first text");
        let hold = fixture
            .store
            .capture_source_hold(&fixture.binding(), wide())
            .unwrap();
        let before = fixture.store.verified_object_reads_for_test();
        let (start, started) = mpsc::channel();
        let (locked, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (done, wait) = mpsc::channel();
        let store = Arc::clone(&fixture.store);
        let fixture = &fixture;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                if started.recv_timeout(Duration::from_secs(2)).is_ok() {
                    store.with_readers_held_for_test(|| {
                        locked.send(()).unwrap();
                        released.recv_timeout(Duration::from_secs(3)).unwrap();
                    });
                }
            });
            scope.spawn(move || {
                let budget = timed_budget();
                let snapshot_hook = move || {
                    start.send(()).unwrap();
                    ready.recv_timeout(Duration::from_secs(1)).unwrap();
                };
                let result = if status {
                    let mut snapshot_hook = Some(snapshot_hook);
                    fixture
                        .store
                        .source_hold_status_with_hook_for_test(
                            &budget,
                            &hold.binding,
                            &hold.hold_id,
                            wall_ms(),
                            |phase| {
                                if phase == SourceHoldCheckPhase::AfterSnapshot {
                                    snapshot_hook.take().unwrap()();
                                }
                            },
                        )
                        .map(|_| ())
                        .map_err(SourceExportError::from)
                } else {
                    KernelStore::with_source_export_after_snapshot_hook_for_test(
                        snapshot_hook,
                        || {
                            fixture.store.export_source_page_within_budget(
                                &budget,
                                &hold.binding,
                                &hold.hold_id,
                                wall_ms(),
                                ExportWindow::Snapshot,
                                None,
                                page_bounds(),
                            )
                        },
                    )
                    .map(|_| ())
                };
                done.send((result, fixture.store.verified_object_reads_for_test()))
                    .unwrap();
            });
            let result = wait.recv_timeout(Duration::from_secs(1));
            release.send(()).unwrap();
            let (result, after) =
                result.expect("second acquisition refuses while the pool stays held");
            assert_eq!(
                result,
                Err(SourceExportError::Kernel(KernelError::Deadline))
            );
            assert_eq!(
                after - before,
                1,
                "artifact read finishes before the refused recheck"
            );
        });
    }
}

#[test]
fn bounded_release_and_reconciliation_close_only_their_holds() {
    let fixture = Fixture::open();
    let first = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let second = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let budget = EvalBudget::unbounded();
    fixture
        .store
        .release_source_hold_within_budget(&budget, &first.binding, &first.hold_id, wall_ms())
        .unwrap();
    assert!(
        fixture
            .store
            .reconcile_source_holds_within_budget(&budget, CONSUMER, wall_ms())
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .store
            .source_hold_status_within_budget(&budget, &second.binding, &second.hold_id, wall_ms())
            .is_ok()
    );
    let fixture = fixture.reopen();
    assert_eq!(
        fixture
            .store
            .reconcile_source_holds_within_budget(&budget, CONSUMER, wall_ms())
            .unwrap(),
        vec![second.hold_id]
    );
    assert_eq!(
        fixture.count("SELECT count(*) FROM capture_pins WHERE released_at IS NOT NULL"),
        2
    );
}

#[test]
fn database_identity_is_authoritative_and_survives_reopen() {
    let fixture = Fixture::open();
    let budget = EvalBudget::unbounded();
    let target = fixture
        .store
        .capture_commit_read_target_within_budget(&budget)
        .unwrap();
    let identity = fixture
        .store
        .database_incarnation_id_within_budget(&budget)
        .unwrap();
    let stored: String = fixture
        .inspect()
        .query_row(
            "SELECT database_incarnation_id FROM kernel_format_marker WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(identity, stored);
    assert_eq!(
        fixture
            .store
            .capture_commit_read_target_within_budget(&budget)
            .unwrap()
            .incarnation,
        target.incarnation
    );
    let fixture = fixture.reopen();
    assert_eq!(
        fixture
            .store
            .database_incarnation_id_within_budget(&budget)
            .unwrap(),
        identity
    );
    assert_ne!(
        fixture
            .store
            .capture_commit_read_target_within_budget(&budget)
            .unwrap()
            .incarnation,
        target.incarnation
    );
}
