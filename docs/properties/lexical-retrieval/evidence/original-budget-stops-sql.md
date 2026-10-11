# original-budget-stops-sql

## Discovery trigger

Ticket #391, acceptance criterion AC6: "Original cancellation/deadline stops
actual SQL within approved bounds, without false partial/direct completion",
together with AC5's "No implicit expansion or fresh lane deadline is allowed."
The RP2.3 property bundle is unavailable in this repository, so the catalog
reconstructs the record from the ticket and verifies it against the #391
branch. PR #967 lists AC6 as met by existing tests and names
`query_route.rs:74` with the lexical interrupt tests.

## Evidence trail

Route-level budget, `crates/daemon/src/query_route.rs`:

- `:1328` `execute` calls `admit_lanes` (`:1353`) then `select` (`:1462`). Both
  take one `&SharedBudget`; no phase derives another.
- `:470` `check` returns `exhaustion(budget)` (`:463`), which maps
  `Exhaustion::Cancelled` to `Terminal::Cancelled` and a deadline or an
  unclassified stop to `Terminal::Deadline`.
- `:1369` `projection.read_under(budget, ...)` holds the connection for the
  exact, lexical, and dense phases. Inside it `:1383` through `:1385` run
  `before_phase(Phase::Lexical)`, `check(budget)`, then `lexical_read`
  (`:1075`), which passes `budget.eval()` into `retrieval::lexical::scan`
  and maps `LaneRefusal::Budget` to `exhaustion(budget)`.
- `:1242` `admit_lexical` judges the scanned lane with `budget.eval()` after
  the connection is released.
- `crates/daemon/src/search_projection.rs:406` `read_under` builds
  `Access::ReadUnder` from `budget.deadline()` and `budget.stop_predicate()`
  (`crates/daemon/src/request_budget.rs:119`, true once cancelled or past the
  deadline) and at `:461` calls `store.with_conn_interruptible`.
- `crates/storage/src/lib.rs:453` `with_conn_interruptible` waits for the
  connection under the same `stop`, then `InterruptScope::install` (`:804`)
  registers it as the SQLite progress handler every `READ_INTERRUPT_STEPS`
  (`:191`, 1,000 VM steps). `is_interrupted` (`:780`) classifies the result.

Lexical lane, `crates/retrieval/src/lexical/retrieve.rs`:

- `:317` `retrieve` and `:349` `scan` take the caller's `&EvalBudget`. `scan`
  checks it first (`:355` through `:357`) and refuses `BudgetExhausted` before
  any probe.
- Every statement loop calls `budget.check()` per row: `count_probe` (`:566`,
  check at `:572`), `scan_whole` (`:623`, check at `:638`), `ranked_rows`
  (`:712`, check at `:726`), and `scan_common` (`:828`, check at `:844`).
- `crates/retrieval/src/scan.rs` maps a SQLite `OperationInterrupted` error to
  `ScanStop::Budget`; `retrieve.rs:400` and `:452` turn it into
  `exhausted_scan`, so an interrupt and an exhausted budget share one exit.
- `:925` `judge_batch` maps `KernelError::Deadline` from the kernel reader to
  `IncompleteReason::BudgetExhausted`; `admit_inner` (`:482`) returns
  `exhausted` when the budget ends after revalidation.
- `:523` `exhausted`: zero completed probes refuse
  `RetrievalRefusal::BudgetExhausted` (`:525`); otherwise contributions clear
  and the result is `Completion::Incomplete(BudgetExhausted)` (`:527`, `:528`).

Tests:

- `crates/daemon/tests/query_route.rs:74`
  `cancellation_and_deadline_are_observed_in_every_phase`. For each of the ten
  phases in `ALL_PHASES` (`crates/daemon/tests/support/query_route.rs:58`,
  Probes through Response, including `Phase::Lexical`) the `before_phase` hook
  cancels the token, and the run must end `Terminal::Cancelled` with that phase
  last reached. A second loop derives a 600 ms budget (`request_budget`,
  `:148`) and sleeps 700 ms in the hook; the run must end `Terminal::Deadline`
  at that phase. The fixture runs `execute` with `DenseLane::Undeclared`
  (`:388`).
- `crates/daemon/tests/query_route.rs:112`
  `claim_validation_stops_waiting_for_a_held_connection_when_the_budget_ends`
  holds the projection connection for 3 s during claim validation and asserts
  a 400 ms deadline and a 200 ms cancellation each end the wait in under 2 s.
- `crates/retrieval/tests/lexical_retrieval.rs:1407`
  `an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion`
  projects 2,500 `bulk parse` rows, runs under `with_conn_interruptible` with a
  stop closure that trips on its third poll, and asserts
  `Err(RetrievalRefusal::BudgetExhausted)`. A non-stopping control completes
  with 2,504 scanned rows. A third run with `scan_rows` 1 trips on the
  eleventh poll while `parse` is counted: one probe consumed, counted rows
  above zero, zero scanned rows, `Incomplete(BudgetExhausted)`, no
  contributions, and the `EvalBudget` itself stays unexhausted.
- `:2049`
  `an_interrupt_anywhere_in_counting_ranking_or_a_common_scan_is_budget_exhaustion`
  uses the 20,001-row threshold corpus (`project_thresholds`, `:1549`). For
  `t20001` tight and wide, `t20000`, and `c15000 d15000`, it measures the
  total poll count of a complete run, then stops at eight evenly spaced poll
  counts. Each stop yields `Err(BudgetExhausted)` or
  `Incomplete(BudgetExhausted)` with no contributions, and the connection
  serves an unbudgeted request afterward.
- `:1516` `a_held_kernel_reader_does_not_outlive_the_budget` holds kernel
  readers for 3 s and asserts a 300 ms deadline returns within 1.5 s with
  `Incomplete(BudgetExhausted)` and zero batches.

## Failure scenario

A lane that derives its own deadline, or a statement that runs without the
progress handler, keeps the connection and the SQLite VM busy after the
caller gave up. The pooled connection stays unavailable to the next request,
a cancelled client's work completes anyway, and a partial result could be
returned as if complete.

## Timing windows and dependencies

Per phase: the hook runs before each phase's `check`, so the route test proves
the entry check, with a 100 ms margin between the 600 ms budget and the 700 ms
sleep. In flight: the progress handler fires every 1,000 VM steps, so an
interrupt lands within that many steps of the stop predicate turning true. The
lexical tests sweep the poll count from zero to the full run in eighths, so
the interrupt lands in counting, ranking, and a common scan. Kernel reader
waits stop at `budget.acquire_limit()`.

## What a test must construct

A projection populated from a live kernel and a `RequestBudget` derived with
a cancellation token and a remaining duration; a `before_phase` hook that
cancels or sleeps in the chosen phase. For in-flight stops, a connection
checked out through `with_conn_interruptible` with a counting stop closure,
enough rows that a run polls more than eight times, and a bound set that puts
most of the work in a count, a ranked read, or a common scan.

## Investigation log

### Q: Does the route test prove an interrupt inside a lexical statement?

- Sources examined: `crates/daemon/tests/query_route.rs:74` through `:110`;
  `crates/daemon/src/query_route.rs:1383` through `:1385`.
- Findings: the hook runs in `before_phase`, before `check(budget)` and before
  `lexical_read`. The cancel and the sleep therefore end the request at the
  phase entry check, not inside a MATCH statement. In-flight interruption is
  proved only by the retrieval tests at `lexical_retrieval.rs:1407` and
  `:2049`, which install the handler through `with_conn_interruptible`, the
  same storage entry `read_under` uses in production.
- Missing evidence: a route-level test that cancels while a lexical statement
  is stepping.
- Conclusion: resolved with answer - the two layers together cover the claim;
  the route proves phase propagation and the retrieval tests prove the
  statement interrupt on the same storage path.

### Q: Is the dense lane covered by the per-phase test?

- Sources examined: `crates/daemon/tests/support/query_route.rs:388`.
- Findings: `Fixture::run` passes `DenseLane::Undeclared`, so `Phase::Dense`
  is entered and checked but runs no oracle.
- Missing evidence: a per-phase cancellation run with a declared dense lane.
- Conclusion: unresolved, needs a dense-declared fixture; outside this
  lexical record's scope.
