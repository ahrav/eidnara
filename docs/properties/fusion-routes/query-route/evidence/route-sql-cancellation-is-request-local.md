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
- `crates/daemon/src/search_projection.rs` `read_under` passes a fresh stop
  predicate per request and maps an engine error under an exhausted budget to
  the store's deadline error, so an interrupted statement is reported as
  exhaustion.
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

### Q: How is an interrupt distinguished from an engine failure once the callback has mapped it to a string?

- Sources examined: `ProjectionError::Sqlite(String)`; `store_error` mapping
  in storage; `read_under`.
- Findings: the callback's error type loses the rusqlite kind. The progress
  handler fires only when the stop predicate holds, so an engine error under
  an exhausted budget is the interrupt or a read whose result is no longer
  authoritative.
- Missing evidence: none.
- Conclusion: resolved with answer - report exhaustion for any engine error
  once the budget is exhausted.
