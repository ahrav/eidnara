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

- Sources examined: `crates/storage/src/lib.rs` negative control.
- Findings: a leaked handler interrupts the next read there.
- Missing evidence: none.
- Conclusion: resolved with answer - yes.
