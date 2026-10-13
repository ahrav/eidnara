# dense-reused-connection-stays-isolated

## Discovery trigger

RP2.6 and #620 AC2: late cancellation of request A must not interrupt
request B on a reused connection.

## Evidence trail

- `SearchProjection::read_under` runs through
  `SqliteStore::with_conn_interruptible`, which installs the stop predicate
  for one read transaction and clears it before the transaction ends.

## Failure scenario

A progress handler left installed interrupts the next caller's statements.

## Timing windows and dependencies

A's token cancelled during B's ranking.

## What a test must construct

- Two sequential requests on one projection, the first cancelled, the second
  observing a late cancellation of the first.

## Investigation log

### Q: Is the leak detectable?

- Sources examined: `crates/storage/src/lib.rs` negative control;
  `crates/storage/src/lib.rs:474`, where every guarded read installs its own
  progress handler; `crates/daemon/tests/query_route_compressed.rs`
  `a_leaked_progress_handler_fails_a_plain_read_that_the_next_guarded_request_masks`.
- Findings: a leaked handler interrupts the next read there. At the route, the
  next request's guarded read replaces a leaked handler when it starts, so that
  request completes over the leak. A plain read run before it is interrupted
  by the leak, so the route witness runs one after each abnormal request. With
  the unwind cleanup in `InterruptScope`'s `Drop` removed, the route witness
  fails at that plain read.
- Missing evidence: none.
- Conclusion: resolved with answer - yes, by a plain read that precedes the
  next guarded read.
