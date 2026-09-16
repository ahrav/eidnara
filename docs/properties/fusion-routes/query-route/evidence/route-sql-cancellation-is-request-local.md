# route-sql-cancellation-is-request-local

## Discovery trigger

RP2.7 requires SQLite cancellation scoped to the request's connection
ownership interval: the progress handler is removed before the connection is
reused, and a late cancellation of one request never interrupts another
request on the shared connection.

## Evidence trail

- `crates/storage/src/lib.rs` `read_on` installs `InterruptScope` after the
  read transaction begins and clears it before the callback scope is released
  and the transaction finishes; `Drop` clears it on unwind with
  `progress_handler(0, None)`. `is_interrupted` documents the scope as the only
  source of `SQLITE_INTERRUPT` on a store connection.
- `crates/retrieval/src/lib.rs` maps a `SQLITE_INTERRUPT` failure to
  `ProjectionError::Interrupted` through `storage::is_interrupted`, so the
  interrupt survives the callback's error type; `SearchProjection::run` in
  `crates/daemon/src/search_projection.rs` returns that one variant as the
  store's deadline error on every access mode while every other engine error
  keeps its own class, and `read_under` passes a fresh stop predicate per
  request. `search_catchup::classify` and its quarantine-capable callers
  therefore never receive the variant.
- `crates/daemon/tests/request_budget_reads.rs` cancels one request's read
  and then runs a fresh request's read and a plain bounded read on the same
  connection.
- `crates/storage/src/lib.rs` test
  `a_leaked_progress_handler_interrupts_the_next_read_on_the_connection`
  installs a handler outside the scope and shows the next read is interrupted.

## Failure scenario

A handler that survives the transaction fires on the next request's `BEGIN`
or statements, failing an unrelated request with a spurious deadline. An
interrupted statement surfacing as an engine error would be read as
corruption rather than as the caller's own cancellation.

## Timing windows and dependencies

Cancellation raised during a statement; cancellation raised after the read
returned but before the next acquisition; a callback that unwinds with the
handler installed.

## What a test must construct

- A file-backed projection, a long recursive scan, and a cancel from another
  thread.
- A second request on the same connection after the cancelled one.
- The negative control: a leaked handler and the next read's failure.

## Investigation log

### Q: How is an interrupt distinguished from an engine failure once the callback has mapped it to `ProjectionError`?

- Sources examined: `ProjectionError::Sqlite(String)`; `storage::is_interrupted`;
  `retrieval::scan::ScanStop`, which already classifies interrupts before
  converting; `search_catchup::classify`, which quarantines integrity errors.
- Findings: a string-typed error would force a text match or a budget-state
  guess, and a guess would relabel a real corruption or I/O failure racing the
  deadline as a retryable deadline.
- Missing evidence: none.
- Conclusion: resolved with answer - `From<rusqlite::Error>` yields a
  dedicated `Interrupted` variant, and only that variant becomes the deadline
  error.

### Q: Where is `Interrupted` turned into the deadline error?

- Sources examined: `SearchProjection::read_under` and `SearchProjection::run`;
  `search_catchup::classify`, whose `Refusal::Storage` arm the catch-up,
  embedding-dispatch, and embedding-publication paths turn into a storage
  quarantine.
- Findings: a rewrite confined to `read_under` leaves `classify` as a second
  encoding of the same event that quarantines the projection, so any later
  interruptible access reaching a classifying consumer would quarantine on a
  cancellation. `run` is the one exit every access mode shares.
- Missing evidence: none.
- Conclusion: resolved with answer - `run` performs the rewrite for every
  access mode; `an_interrupted_statement_is_the_deadline_error_on_every_access_mode`
  holds it.
