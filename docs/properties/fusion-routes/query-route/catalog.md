# RP2.7 query-route budget and cancellation properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: the `rp27/u1-identity-contract` branch head this change was authored
against, itself on `main` at `8e0491225a7292ef077c675d44b94f94a24041d3`.
Method: `../../METHOD.md` and `property-discovery-and-catalog`.

Source: the RP2.7 specification
([#630](https://github.com/ahrav/eidnara/issues/630)) and its local companion
bundle, whose query-route catalog proposed these slugs as unexercised
`test-only` obligations. The RP2.7.U3a ticket
([#639](https://github.com/ahrav/eidnara/issues/639)) lands the budget and
cancellation bridge records. Authorization, degradation, and terminal records
enter this file with RP2.7.U3b and U3c.

This part owns the request budget: one absolute `EvalBudget` derived at
handler entry, tracked blocking work, and the interruptible search-projection
read. The route that composes lanes over it is a later ticket.

Parent Q4 decisions recorded here: the remaining-duration request field is
`remaining_ms`, a positive integer of milliseconds, clamped to a route-supplied
ceiling that fails closed when absent; the budget is owned by an RAII guard
whose drop cancels it; connection acquisition observes cancellation through
the same stop predicate as SQLite statements, so the deadline-only acquisition
gap is closed rather than documented; every blocking phase uses the host's
tracked primitive.

## Observation contract

Each record carries its own reachability label and the evidence for it; a
later ticket that gives a record a production caller relabels that record
alone. Observation points: `crates/daemon/src/request_budget.rs`,
`SearchProjection::read_under` in `crates/daemon/src/search_projection.rs`,
`SqliteStore::with_conn_interruptible` in `crates/storage/src/lib.rs`, and the
host's `RequestCtx::run_blocking`. The host-level witnesses run a real host
with a real client in `crates/daemon/src/request_budget/host_tests.rs`.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [route-budget-is-derived-once-before-queue-wait](#route-budget-is-derived-once-before-queue-wait) | safety | test-only | always | active | high |
| [route-sql-cancellation-is-request-local](#route-sql-cancellation-is-request-local) | safety | test-only | always | active | high |
| [route-permits-and-pins-outlive-client-cancellation](#route-permits-and-pins-outlive-client-cancellation) | safety | test-only | always | active | medium |

## Records

### route-budget-is-derived-once-before-queue-wait

Type: safety
Reachability: test-only - `RequestBudget::derive` has no caller outside
`crates/daemon/src/request_budget.rs`, its `host_tests.rs`, and
`crates/daemon/tests/request_budget_reads.rs`; no daemon handler derives a
budget at this head.
Status: active
Exercised: yes - `crates/daemon/src/request_budget.rs` unit tests
`the_remaining_duration_is_clamped_to_the_approved_ceiling`,
`a_request_without_an_approved_ceiling_or_a_remaining_duration_is_refused`,
`dropping_the_guard_cancels_every_clone_and_the_stop_predicate`; and
`crates/daemon/tests/request_budget_reads.rs`
`an_elapsed_remaining_duration_interrupts_a_held_read_the_same_way`,
`a_cancelled_budget_leaves_the_connection_wait_before_the_holder_releases`.
Guarantee: One absolute budget is derived at handler entry from the request's
cancellation and its clamped `remaining_ms`; every later stage, including the
connection acquisition wait and SQLite VM steps, observes that same deadline
and flag, and no stage re-derives a relative budget.
Check: `always` - the guard, every `SharedBudget` clone, the `EvalBudget`
inside each clone, and the stop predicate a blocking thread carries report one
identical deadline; the `EvalBudget` is never handed out alone, because host
cancellation reaches its flag only through `SharedBudget::exhaustion` or the
stop predicate; a missing ceiling, a missing or zero `remaining_ms`, or an
already-cancelled request is refused before any work; an acquisition wait
ends on cancellation before the holder releases and before the deadline.
`always` because the property must hold on every derivation and every clone.
Fault/timing angle: The acquisition wait while another reader holds the
connection; the moment between `derive` and the first blocking phase.
Required faults and enabling state: A held connection on another thread; a
`remaining_ms` above the ceiling; a `remaining_ms` that elapses during a scan.
Confidence: high - [evidence](evidence/route-budget-is-derived-once-before-queue-wait.md).
The deadline is one `Instant` copied into every clone; the stop predicate
folds the host's cancellation into the same flag.
Existing check: `crates/kernel/tests/kernel_source_budgets.rs`
`bounded_capture_export_complete_commits_and_ack_preserve_fencing` cancels a
budget on drop across the acknowledgement boundary, not across a running
scan, status unaudited; `crates/retrieval/tests/lexical_retrieval.rs`
`an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion`
drives the storage scope directly, status unaudited.
Impact: A stage with its own fresh budget could outlive the caller's deadline,
and a transport timeout could stand in for the caller's remaining duration.
Open questions:

- A SQLite busy wait inside the read runs under the store's standing 5 s
  `BUSY_TIMEOUT` (`crates/storage/src/lib.rs`), which neither the deadline
  nor the stop predicate shortens; the progress handler polls only between VM
  steps. In WAL mode a reader waits only during recovery or against an
  exclusive-locking-mode connection, and an attempt to construct that wait
  against a store that already holds the wal-index was refused with
  `DatabaseBusy` on the blocker side. Unresolved, needs a reproducible busy
  reader before the write path's `with_busy_timeout_until` is applied here.

### route-sql-cancellation-is-request-local

Type: safety
Reachability: test-only - `SearchProjection::read_under` is the only
production caller of `with_conn_interruptible`, and `read_under` itself is
called only from `crates/daemon/tests/request_budget_reads.rs` and
`crates/daemon/src/request_budget/host_tests.rs`.
Status: active
Exercised: yes - `crates/daemon/tests/request_budget_reads.rs`
`a_later_request_on_the_same_connection_is_not_interrupted_by_a_prior_cancellation`,
`a_cancellation_after_a_successful_read_does_not_interrupt_a_later_plain_read`,
`cancelling_the_request_interrupts_a_held_read_and_reports_exhaustion`,
`an_interrupted_statement_is_the_deadline_error_on_every_access_mode`; and
`crates/storage/src/lib.rs`
`an_interruptible_read_stops_a_running_statement_and_a_later_read_is_untouched`
plus the negative control
`a_leaked_progress_handler_interrupts_the_next_read_on_the_connection`.
Guarantee: The SQLite progress handler that observes a request's budget is
installed for that request's read only and removed before the transaction
ends and on unwind, so a late cancellation of one request never interrupts a
later request on the shared connection, and an interrupted read reports budget
exhaustion rather than corruption.
Check: `always` - after a cancelled read on the projection connection, a fresh
request's interruptible read and a plain bounded read both complete; the
cancelled read returns the store's deadline error because the engine reported
`ProjectionError::Interrupted`, a variant only the progress handler produces,
and `SearchProjection::run` returns it as the deadline error on every access
mode, so no consumer classifying a projection refusal can quarantine the
projection for a cancellation; the negative control shows that
a handler left installed does interrupt the next read, so the oracle detects a
leak. `always` because
every read on the connection must be free of the previous request's hook.
Fault/timing angle: Cancellation raised while a statement runs; cancellation
raised after the read returned but before the next request acquires the
connection; a callback that unwinds with the handler installed.
Required faults and enabling state: A file-backed `search.sqlite`; a long
recursive scan; a second request on the same connection after cancellation.
Confidence: high - [evidence](evidence/route-sql-cancellation-is-request-local.md).
The storage scope clears the handler before the transaction finishes and in
`Drop`; the daemon read passes a fresh predicate per request.
Existing check: `crates/storage/src/lib.rs`
`an_interruptible_read_stops_a_running_statement_and_a_later_read_is_untouched`,
status unaudited; `crates/kernel/src/budget_tests.rs`
`commit_clears_interrupt_before_sql_and_rearms_it_for_connection_reuse` for the
kernel store, status unaudited.
Impact: A leaked hook would fail an unrelated request with a spurious deadline
or leave an interrupted read reported as an engine failure.
Open questions: None.

### route-permits-and-pins-outlive-client-cancellation

Type: safety
Reachability: test-only - the host witnesses in
`crates/daemon/src/request_budget/host_tests.rs` register a test handler on
a real host; no production route registers a handler that derives a budget
and runs a projection read under it.
Status: active
Exercised: partial - the bridge clauses are covered by
`crates/daemon/src/request_budget/host_tests.rs`
`cancelling_a_suspended_handler_interrupts_the_held_read_and_joins_it_before_settling`,
`the_guard_drop_alone_interrupts_the_held_read_when_the_host_aborts_the_handler`,
and `a_panic_in_tracked_blocking_work_is_typed_and_still_settles`, with the
drop's classification under a concurrent poll covered by
`crates/daemon/src/request_budget.rs`
`a_poll_that_straddles_the_guard_drop_never_reports_a_deadline`; the permit
and pin clauses wait for the route (U3b) and the dense lane (U3c) that hold
them.
Guarantee: After client cancellation the blocking worker is joined before the
request settles; the connection it holds stays held until the interrupted
read returns, then `read_on` releases it as the read completes, before the
worker is joined, so connection ownership ends with the read while
work-tracker ownership ends with the join; a panic inside the blocking
closure surfaces as its typed failure and still settles.
Check: `always` - a real client cancels a request whose handler is suspended
at `run_blocking` with no `select!` arm; the read reports exhaustion at a time
no later than the host's error publication for that channel; the projection
connection is held before cancellation and free after settlement; with the
budget derived from a token the test never cancels, the guard's drop alone
stops the read and classifies it as cancellation, including when a poll of
`SharedBudget::exhaustion` straddles the drop; a panicking closure yields
`BlockingFailure::Panicked` and the client sees one terminal error. `always`
because every cancelled request must drain its blocking work.
Fault/timing angle: The host aborts the handler future while the blocking
read runs; in the drop-only variant the guard's drop is the only path raising
the interrupt; a poll that reads the budget between the drop's two stores.
Required faults and enabling state: A real host and client; a held
search-projection read on the blocking pool; a request cancel frame; a panic
inside tracked work.
Confidence: medium - [evidence](evidence/route-permits-and-pins-outlive-client-cancellation.md).
The ordering witness compares the read's return time with the error
publication hook; permits and pins do not exist on this path yet.
Existing check: `crates/host-runtime/tests/dispatch.rs`
`cancel_waits_for_the_request_blocking_work` and
`a_blocking_work_panic_settles_as_one_internal_error`, status unaudited;
`crates/daemon/src/transform_unit/host_tests.rs`
`request_cancel_waits_for_committed_transform_and_releases_scratch`, status
unaudited.
Impact: Settling before the join would release a connection or permit while
work still uses it, or report a false completion.
Open questions:

- Which route-owned permits and dense pins the census must include is fixed
  when U3b and U3c add them. (needs human input)

## Relationship map

The derivation record is the precondition for the other two: the deadline and
flag it mints are what the progress handler and the acquisition poll observe,
so a re-derived or transport-supplied budget would make the request-local and
join-before-settle checks pass against the wrong deadline. The request-local
record constrains the connection while a request owns it and after its read
returns; the join record constrains the worker and the ledger charge after the
host aborts the handler, and hands the connection back through the same
`read_on` release the request-local record relies on. Neither later record
proves the derivation is unique, and the derivation record says nothing about
what a leaked handler or an early settlement would do; each guarantee needs
its own check. The permit and pin clauses of the join record wait on state the
route and dense lane add, and will not change the other two records.
