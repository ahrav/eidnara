//! A request's budget ends a search-projection read at the next polled SQLite step; the interrupt never leaks to a later request on the same connection; a caller waiting for a held connection leaves the wait when its budget is cancelled.

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use daemon::request_budget::{Exhaustion, RequestBudget, SharedBudget};
use daemon::search_projection::{SearchProjection, SearchProjectionError};
use host_runtime::CancelSignal;
use retrieval::ProjectionError;
use storage::{GuardedConn, StoreError};
use tokio_util::sync::CancellationToken;

/// The 200-million-step recursive query keeps the read active for interruption tests; the progress handler polls every 1,000 VM steps.
const LONG_SCAN: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000000) SELECT count(*) FROM c";
const SHORT_SCAN: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 1000) SELECT count(*) FROM c";
/// `MEDIUM_SCAN` gives a leaked progress handler multiple polling opportunities.
const MEDIUM_SCAN: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000) SELECT count(*) FROM c";
const CEILING: Duration = Duration::from_secs(30);

fn scan(sql: &str) -> impl FnOnce(&GuardedConn<'_>) -> Result<i64, ProjectionError> + '_ {
    move |conn| Ok(conn.query_row(sql, [], |row| row.get(0))?)
}

fn derive(remaining_ms: u64) -> (CancellationToken, RequestBudget) {
    let token = CancellationToken::new();
    let budget = RequestBudget::derive(
        CancelSignal::observing(token.clone()),
        Some(remaining_ms),
        Some(CEILING),
    )
    .unwrap();
    (token, budget)
}

fn is_deadline(error: &SearchProjectionError) -> bool {
    matches!(error, SearchProjectionError::Store(StoreError::Deadline))
}

/// The first receiver fires from inside the read callback, after acquisition and interrupt-scope
/// installation, so a cancellation raised after it is observed by the running statement.
type ScanOutcome = (Instant, Result<i64, SearchProjectionError>);

fn held_scan(
    projection: &Arc<SearchProjection>,
    shared: SharedBudget,
) -> (mpsc::Receiver<()>, mpsc::Receiver<ScanOutcome>) {
    let (ready_tx, ready) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let projection = Arc::clone(projection);
    thread::spawn(move || {
        let result = projection.read_under(&shared, |conn| {
            ready_tx.send(()).unwrap();
            scan(LONG_SCAN)(conn)
        });
        let _ = tx.send((Instant::now(), result));
    });
    (ready, rx)
}

fn open() -> (tempfile::TempDir, Arc<SearchProjection>) {
    let dir = tempfile::tempdir().unwrap();
    let projection = Arc::new(SearchProjection::open(dir.path()).unwrap());
    (dir, projection)
}

#[test]
fn cancelling_the_request_interrupts_a_held_read_and_reports_exhaustion() {
    let (_dir, projection) = open();
    let (token, budget) = derive(CEILING.as_millis() as u64);
    let started = Instant::now();
    let (ready, rx) = held_scan(&projection, budget.shared().clone());
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        rx.try_recv().is_err(),
        "the scan must still be running when cancellation arrives"
    );
    token.cancel();
    let (finished, result) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(is_deadline(&result.unwrap_err()));
    assert!(
        finished - started < Duration::from_secs(3),
        "the interrupt must land within the polled step bound, not at the deadline"
    );
    assert_eq!(budget.exhaustion(), Some(Exhaustion::Cancelled));
}

#[test]
fn an_elapsed_remaining_duration_interrupts_a_held_read_the_same_way() {
    let (_dir, projection) = open();
    let (_token, budget) = derive(200);
    let started = Instant::now();
    let (_ready, rx) = held_scan(&projection, budget.shared().clone());
    let (finished, result) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(is_deadline(&result.unwrap_err()));
    assert!(finished >= budget.deadline());
    assert!(finished - started < Duration::from_secs(3));
    assert_eq!(budget.exhaustion(), Some(Exhaustion::Deadline));
}

#[test]
fn an_exhausted_budget_is_refused() {
    let (_dir, projection) = open();
    let (token, budget) = derive(1_000);
    token.cancel();
    assert!(is_deadline(
        &projection
            .read_under(budget.shared(), scan(SHORT_SCAN))
            .unwrap_err()
    ));
    let (_token, dropped) = derive(1_000);
    let shared = dropped.shared().clone();
    drop(dropped);
    assert!(is_deadline(
        &projection
            .read_under(&shared, scan(SHORT_SCAN))
            .unwrap_err()
    ));
}

#[test]
fn a_later_request_on_the_same_connection_is_not_interrupted_by_a_prior_cancellation() {
    let (_dir, projection) = open();
    let (token, prior) = derive(CEILING.as_millis() as u64);
    let (ready, rx) = held_scan(&projection, prior.shared().clone());
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    token.cancel();
    assert!(is_deadline(
        &rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .1
            .unwrap_err()
    ));
    assert!(prior.is_exhausted());
    let (_next_token, next) = derive(CEILING.as_millis() as u64);
    assert_eq!(
        projection
            .read_under(next.shared(), scan(SHORT_SCAN))
            .unwrap(),
        1000
    );
    assert_eq!(
        projection
            .read_within(Instant::now() + Duration::from_secs(1), scan(SHORT_SCAN))
            .unwrap(),
        1000
    );
}

/// The plain read runs before any fresh `read_under`: a replacement handler would mask the
/// handler left behind by a successful `read_under`.
#[test]
fn a_cancellation_after_a_successful_read_does_not_interrupt_a_later_plain_read() {
    let (_dir, projection) = open();
    let (token, prior) = derive(CEILING.as_millis() as u64);
    assert_eq!(
        projection
            .read_under(prior.shared(), scan(SHORT_SCAN))
            .unwrap(),
        1000
    );
    token.cancel();
    assert!(prior.is_exhausted());
    assert_eq!(
        projection
            .read_within(Instant::now() + Duration::from_secs(10), scan(MEDIUM_SCAN))
            .unwrap(),
        200_000
    );
}

/// An engine interrupt is the budget's verdict on every access mode, so no consumer classifying a
/// `ProjectionError` refusal ever sees `Interrupted` and quarantines the projection for a cancellation.
#[test]
fn an_interrupted_statement_is_the_deadline_error_on_every_access_mode() {
    let (_dir, projection) = open();
    let (_token, budget) = derive(CEILING.as_millis() as u64);
    let far = || Instant::now() + Duration::from_secs(5);
    let interrupted =
        |_: &GuardedConn<'_>| -> Result<(), ProjectionError> { Err(ProjectionError::Interrupted) };
    let outcomes = [
        ("read", projection.read(interrupted)),
        ("read_within", projection.read_within(far(), interrupted)),
        (
            "read_under",
            projection.read_under(budget.shared(), interrupted),
        ),
        ("write", projection.write(interrupted)),
        ("write_within", projection.write_within(far(), interrupted)),
    ];
    for (mode, outcome) in outcomes {
        let error = outcome.unwrap_err();
        assert!(is_deadline(&error), "{mode}: {error:?}");
    }
}

#[test]
fn a_cancelled_budget_leaves_the_connection_wait_before_the_holder_releases() {
    let (_dir, projection) = open();
    let (holding, released) = mpsc::channel::<()>();
    let (ready_tx, ready) = mpsc::channel::<()>();
    let holder = Arc::clone(&projection);
    let hold = thread::spawn(move || {
        holder
            .read(|_| {
                ready_tx.send(()).unwrap();
                let _ = released.recv_timeout(Duration::from_secs(60));
                Ok(())
            })
            .unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();

    let (token, budget) = derive(CEILING.as_millis() as u64);
    let shared = budget.shared().clone();
    let waiter = Arc::clone(&projection);
    let (done_tx, done) = mpsc::channel();
    thread::spawn(move || {
        let started = Instant::now();
        let result = waiter.read_under(&shared, scan(SHORT_SCAN));
        let _ = done_tx.send((started.elapsed(), result));
    });
    thread::sleep(Duration::from_millis(100));
    assert!(
        done.try_recv().is_err(),
        "the waiter must be blocked on acquisition"
    );
    token.cancel();
    let (waited, result) = done.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(is_deadline(&result.unwrap_err()));
    assert!(
        waited < Duration::from_secs(2),
        "acquisition must end on cancellation, not at the deadline"
    );
    assert_eq!(budget.exhaustion(), Some(Exhaustion::Cancelled));
    holding.send(()).unwrap();
    hold.join().unwrap();
    assert_eq!(projection.read(scan(SHORT_SCAN)).unwrap(), 1000);
}
