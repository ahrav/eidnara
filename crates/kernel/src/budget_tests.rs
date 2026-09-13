use super::*;
use crate::applicability::EvalBudget;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const LONG_SCAN: &str = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n";

#[test]
fn sqlite_cancelled_rollback_probe() {
    for steps in [1, 1_000] {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE probe(value INTEGER)")
            .unwrap();
        let tx = connection.transaction().unwrap();
        tx.execute("INSERT INTO probe VALUES(1)", []).unwrap();
        assert!(
            Transaction::new_unchecked(&tx, TransactionBehavior::Deferred).is_err(),
            "SQLite rejects a nested BEGIN"
        );
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        tx.progress_handler(
            steps,
            Some(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                true
            }),
        )
        .unwrap();
        let rollback = tx.rollback();
        let autocommit = connection.is_autocommit();
        connection
            .progress_handler(0, None::<fn() -> bool>)
            .unwrap();
        println!(
            "steps={steps} rollback={rollback:?} autocommit={autocommit} callbacks={}",
            calls.load(Ordering::SeqCst)
        );
        assert_eq!(
            rollback
                .as_ref()
                .err()
                .and_then(rusqlite::Error::sqlite_error_code),
            (steps == 1).then_some(rusqlite::ErrorCode::OperationInterrupted)
        );
        assert_eq!(autocommit, steps == 1_000);
        assert_eq!(calls.load(Ordering::SeqCst) > 0, steps == 1);
        if !autocommit {
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM probe", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            connection.execute_batch("ROLLBACK").unwrap();
        }
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM probe", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn cancellation_does_not_relabel_an_unknown_commit_error() {
    let budget = EvalBudget::unbounded();
    let limit = budget.acquire_limit();
    let result: Result<(), KernelError> = limit.run(|| {
        budget.cancel();
        Err(KernelError::Io)
    });
    assert_eq!(result, Err(KernelError::Io));
}

#[test]
fn exhausted_limits_refuse_free_reader_and_writer_guards() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    for limit in [
        AcquireLimit::until(Instant::now()),
        AcquireLimit::new(None, Some(Arc::new(AtomicBool::new(true)))),
    ] {
        assert_eq!(
            store.lock_reader_within(&limit).unwrap_err(),
            KernelError::Deadline
        );
        assert_eq!(
            store.lock_writer_within(&limit).unwrap_err(),
            KernelError::Deadline
        );
        assert!(store.writer.try_lock().is_ok());
        assert!(store.readers.iter().all(|reader| reader.try_lock().is_ok()));
    }
}

#[test]
fn cancellation_after_mutex_acquisition_refuses_sql_and_releases_the_guard() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    let budget = EvalBudget::unbounded();
    let limit = budget.acquire_limit();
    let writer = store.lock_writer_within(&limit).unwrap();
    budget.cancel();
    assert_eq!(
        LimitedConnection::new(writer, &limit).err(),
        Some(KernelError::Deadline)
    );
    let writer = store.writer.try_lock().unwrap();
    assert_eq!(
        writer
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        BUSY_TIMEOUT_MS
    );
}

#[test]
fn sql_progress_cancels_scan_and_cleans_pooled_connection_on_error_and_unwind() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    let budget = EvalBudget::unbounded();
    let limit = budget.acquire_limit();
    let cancel = budget.clone();
    let result: Result<(), KernelError> = limit.run(|| {
        let reader = store.reader_with_limit(&limit)?;
        reader
            .create_scalar_function(
                "cancel_budget",
                1,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    cancel.cancel();
                    Ok(1i64)
                },
            )
            .unwrap();
        reader
            .query_row(
                "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000)
             SELECT sum(cancel_budget(x)) FROM n",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(super::super::map_sqlite)?;
        panic!("cancelled scan completed");
    });
    assert_eq!(result, Err(KernelError::Deadline));
    assert!(budget.is_exhausted(), "SQL function ran");
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let budget = EvalBudget::unbounded();
        let _reader = store.reader_with_limit(&budget.acquire_limit()).unwrap();
        budget.cancel();
        panic!("unwind with installed handler");
    }));
    assert!(unwind.is_err());
    let successful = EvalBudget::unbounded();
    {
        let reader = store
            .reader_with_limit(&successful.acquire_limit())
            .unwrap();
        assert_eq!(
            reader
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    successful.cancel();
    for reader in &store.readers {
        let reader = reader.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            reader
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            BUSY_TIMEOUT_MS
        );
        assert_eq!(
            reader
                .query_row(LONG_SCAN, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            50_005_000
        );
    }
}

#[test]
fn precommit_exhaustion_rolls_back_and_restores_writer_settings() {
    for unwind in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = KernelStore::open(root.path()).unwrap();
        let budget = EvalBudget::unbounded();
        let limit = budget.acquire_limit();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            limit.run(|| {
                let mut writer = store.writer_with_limit(&limit)?;
                writer
                    .execute_batch("CREATE TEMP TABLE budget_probe(value INTEGER)")
                    .unwrap();
                let tx = writer.transaction(TransactionBehavior::Immediate)?;
                tx.execute("INSERT INTO budget_probe VALUES(1)", [])
                    .unwrap();
                limit.clone().set_progress_handler(&tx, 1).unwrap();
                budget.cancel();
                assert!(!unwind, "unwind with cancelled transaction");
                tx.commit()
            })
        }));
        if unwind {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), Err(KernelError::Deadline));
        }
        let writer = store.lock_writer().unwrap();
        assert!(writer.is_autocommit());
        assert_eq!(
            writer
                .query_row("SELECT count(*) FROM budget_probe", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            writer
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            BUSY_TIMEOUT_MS
        );
        assert_eq!(
            writer
                .query_row(LONG_SCAN, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            50_005_000
        );
    }
}

#[test]
fn interrupted_fence_read_reports_deadline_not_fence_loss() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    let budget = EvalBudget::unbounded();
    let limit = budget.acquire_limit();
    let mut writer = store.writer_with_limit(&limit).unwrap();
    let tx = writer.transaction(TransactionBehavior::Immediate).unwrap();
    assert_eq!(
        crate::envelope::check_fence(&tx, store.lease_epoch()),
        Ok(())
    );
    // Every opcode polls, so the fence read observes the cancellation itself.
    limit.clone().set_progress_handler(&tx, 1).unwrap();
    budget.cancel();
    assert_eq!(
        crate::envelope::check_fence(&tx, store.lease_epoch()),
        Err(KernelError::Deadline)
    );
    drop(tx);
    drop(writer);
    let fresh = EvalBudget::unbounded();
    let mut writer = store.writer_with_limit(&fresh.acquire_limit()).unwrap();
    let tx = writer.transaction(TransactionBehavior::Immediate).unwrap();
    assert_eq!(
        crate::envelope::check_fence(&tx, store.lease_epoch()),
        Ok(())
    );
    assert_eq!(
        crate::envelope::check_fence(&tx, store.lease_epoch() + 1),
        Err(KernelError::FenceLost)
    );
}

#[test]
fn commit_clears_interrupt_before_sql_and_rearms_it_for_connection_reuse() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    let budget = EvalBudget::unbounded();
    let limit = budget.acquire_limit();
    let mut writer = store.writer_with_limit(&limit).unwrap();
    writer
        .execute_batch("CREATE TEMP TABLE budget_probe(value INTEGER)")
        .unwrap();
    let tx = writer.transaction(TransactionBehavior::Immediate).unwrap();
    tx.execute("INSERT INTO budget_probe VALUES(1)", [])
        .unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    tx.progress_handler(
        1,
        Some(move || {
            observed.store(true, Ordering::SeqCst);
            true
        }),
    )
    .unwrap();
    tx.commit().unwrap();
    assert!(
        !called.load(Ordering::SeqCst),
        "COMMIT executes with no interrupt callback"
    );
    let tx = writer.transaction(TransactionBehavior::Immediate).unwrap();
    tx.execute("INSERT INTO budget_probe VALUES(2)", [])
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        writer
            .query_row("SELECT count(*) FROM budget_probe", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    budget.cancel();
    assert_eq!(
        writer
            .query_row(LONG_SCAN, [], |row| row.get::<_, i64>(0))
            .unwrap_err()
            .sqlite_error_code(),
        Some(rusqlite::ErrorCode::OperationInterrupted)
    );
    assert_eq!(
        writer.transaction(TransactionBehavior::Immediate).err(),
        Some(KernelError::Deadline)
    );
    drop(writer);
    let writer = store.lock_writer().unwrap();
    assert_eq!(
        writer
            .query_row(LONG_SCAN, [], |row| row.get::<_, i64>(0))
            .unwrap(),
        50_005_000
    );
}

#[test]
fn saved_sqlite_busy_timeout_can_expire_before_a_live_caller_budget() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    {
        let writer = store.lock_writer().unwrap();
        writer
            .execute_batch("CREATE TEMP TABLE budget_probe(value INTEGER)")
            .unwrap();
        writer
            .busy_timeout(std::time::Duration::from_millis(5))
            .unwrap();
    }
    let mut external = Connection::open(&store.db_path).unwrap();
    let held = external
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let budget = EvalBudget::new(
        Some(Instant::now() + std::time::Duration::from_secs(2)),
        Arc::new(AtomicBool::new(false)),
    );
    let limit = budget.acquire_limit();
    let result = limit.run(|| {
        let mut writer = store.writer_with_limit(&limit)?;
        let tx = writer.transaction(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO budget_probe VALUES(1)", [])
            .unwrap();
        tx.commit()
    });
    assert_eq!(result, Err(KernelError::Busy));
    assert!(!budget.is_exhausted());
    drop(held);
    let writer = store.lock_writer().unwrap();
    assert_eq!(
        writer
            .query_row("SELECT count(*) FROM budget_probe", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        writer
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
}
