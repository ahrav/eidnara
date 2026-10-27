# p5dep-budget-exhaustion-stops-a-budgeted-kernel-read

HEAD `3a3b916b7`. Surface S3.

## Discovery trigger

Plan R1/KTD2: the P5 pass runs "under its own deadline and byte budget" and
"stop[s] at budget exhaustion". The task names the claim "deadline/interrupt
stop the read". `claim_facts.rs:232-234` claims "reader acquisition and every
statement stop at the deadline or on cancellation"; `slice/read.rs:121`
claims "The wait for a pooled reader stops at `budget`'s deadline or
interrupt." Lens: failure recovery; C7, C8; D2.

## Evidence trail

- `crates/kernel/src/applicability/checkout.rs:145-150` `EvalBudget {
  deadline, interrupt, parent }`; `:153-159` `new`; `:162-168` `linked()`
  (private interrupt, parent flag); `:171-173` `unbounded()`; `:187-193`
  `bounded_by`; `:196-198` `cancel()` "irreversible"; `:203-218`
  `is_exhausted()` checks own flag, then parent flag (and stores into own),
  then deadline (and stores into own); `:235-238` `acquire_limit()` =
  `AcquireLimit::new(deadline, Some(interrupt)).with_parent(parent)`.
- `crates/kernel/src/open.rs:1533-1549` `AcquireLimit::check` ->
  `Deadline` when `should_stop()`; `run` checks then runs.
- `open.rs:1598-1627` `should_stop`: own interrupt, parent (stores into own),
  deadline.
- `open.rs:1581-1594` `install_progress_handler` /
  `set_progress_handler(steps, || self.should_stop())`.
- `open.rs:565-593` `acquire_within`: `limit.check()` before each `try_lock`
  and after; `should_stop()` -> `Deadline` after a full pass; sleeps
  `WRITER_ACQUIRE_POLL` between passes.
- `open.rs:511-517` `lock_reader_within(limit)` -> `acquire_within(readers,
  ..)`.
- `open.rs:1654-1664` `reader_with_limit`: `lock_reader_within` when the
  limit has control, then `limited_connection` ->
  `LimitedConnection::new` (`:1693-1722`): `limit.check()`, zero busy timeout,
  `set_progress_handler(.., SQL_BUDGET_PROGRESS_STEPS)` when the limit has
  control.
- `claim_facts.rs:248` `limit.run`; `:313` `reader_with_limit(limit)`;
  `:326` `limit.check()`; `:545`, `:552` `limit.check()` per claim inside
  `load_claims_served_in_tx`. So claim facts have acquisition, per-claim, and
  in-statement (progress handler) poll points.
- `slice/read.rs:134` `decision_page_within_budget`: `self.lock_reader_within
  (&budget.acquire_limit())?` then a plain `reader.transaction_with_behavior`;
  no `LimitedConnection`, no progress handler, no `limit.check()` after the
  statement. `:241` `decisions_for_objects_as_of_within_budget`: same shape.
- `source_capture.rs:609-612`: the capture job's budget is `EvalBudget::new
  (Some(now + 5 s), interrupt)`; `run` sets the interrupt on cancel (`:662`).
- Tests: `crates/daemon/tests/embedding_dispatch.rs:1102`
  `an_eligibility_read_against_held_kernel_readers_ends_at_the_budget`;
  `crates/retrieval/tests/claims.rs:993`;
  `crates/daemon/tests/search_catchup.rs:1876`. `hold_readers_for_test`
  (`open.rs:614-619`, `test-support`) holds every reader for a duration.

## Failure scenario

For `claim_facts_at_within_budget`: a long statement with the progress
handler not installed would run to completion past the deadline; the P5
pass's "own deadline" would then be a deadline on waiting, not on reading.
For the page API this is the current behavior: a page of `max_rows` rows
runs to completion once the reader is acquired. A P5 design that expects the
budget to cut a page mid-statement is wrong at HEAD; it must size `max_rows`
so a page fits the deadline.

## Timing windows and dependencies

`WRITER_ACQUIRE_POLL` between acquisition passes; `SQL_BUDGET_PROGRESS_STEPS`
VM steps between progress-handler polls. A `linked()` child's own cancel does
not cancel the parent; the parent's cancel exhausts the child at its next
poll.

## What a test must construct

- Hold the pool (`hold_readers_for_test`), call each budgeted API with a
  short deadline; assert `Deadline` within the poll interval of the deadline.
- For claim facts: a deadline that passes after acquisition; a store large
  enough that the statement runs past it; assert `Deadline` and that the
  statement did not complete (SQLite interrupt error mapped to `Deadline`).
- A `linked()` budget whose parent is cancelled mid-read.

## Investigation log

### Q: D2. Does the page API honor the budget inside the statement?

- Sources examined: `slice/read.rs:126-220`, `open.rs:511-517`, `:1654-1722`.
- Findings: no. Only the acquisition wait is bounded; the scan is bounded by
  `max_rows`. The doc comment at `:121` states the wait bound accurately.
- Missing evidence: none for the fact; the P5 author's requirement is open.
- Conclusion: resolved as fact (wait-only); the design question needs human
  input.
