# RP2.7 query-route properties

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
cancellation bridge records. The RP2.7.U3b ticket
([#641](https://github.com/ahrav/eidnara/issues/641)) lands the route records:
authorization, bounds, cancellation in every phase, scope, revalidation,
healthy completion, the closed terminal set, and disable as rollback. The
dense-lane records enter with RP2.7.U3c.

This part owns the request budget (one absolute `EvalBudget` derived at
handler entry, tracked blocking work, and the interruptible search-projection
read) and the `retrieval.query` route in `crates/daemon/src/query_route.rs`
that composes the exact and lexical lanes over it.

Parent Q3 decisions recorded here: `retrieval.query` is handler business
semantics behind the existing `method` envelope, so `docs/host-wire-protocol.md`
is unchanged; the route binding's harness is authoritative, a `harness` claim
that disagrees with it is refused as `unauthorized`, and an absent claim is
accepted. Parent Q5 decisions recorded here: the wire terminal set is
`unauthorized`, `deadline`, `cancelled`, `lane_unavailable`,
`required_context_failure`, and `disabled`; a lane that fails for a reason
other than the budget is reported `unavailable` for that lane with one code
from a closed set (never engine text) and the answer is `degraded`; both lanes
failing, a fused union over its bound, an unreadable projection, or a kernel
failure during revalidation is `lane_unavailable` with a `reason` code naming
the witness; an interrupted statement inside a lane is the budget's own
verdict, `cancelled` or `deadline`; a panic in tracked work is the transport's
`internal_error`; a stopped runtime or closing route is `cancelled`.

Parent Q4 decisions recorded here: the remaining-duration request field is
`remaining_ms`, a positive integer of milliseconds, clamped to a route-supplied
ceiling that fails closed when absent; the budget is owned by an RAII guard
whose drop cancels it; connection acquisition observes cancellation through
the same stop predicate as SQLite statements, so the deadline-only acquisition
gap is closed rather than documented; every blocking phase uses the host's
tracked primitive.

## Reachability and observation contract

Every record here is `test-only`: the route is reachable through the
handler's dispatch only after `Handler::set_query_route_limits` installs an
approved limit set, and no production caller installs one yet. Observation
points: `crates/daemon/src/request_budget.rs`, `SearchProjection::read_under`
in `crates/daemon/src/search_projection.rs`, `SqliteStore::with_conn_interruptible`
in `crates/storage/src/lib.rs`, the host's `RequestCtx::run_blocking`, and
`execute` plus `HandlerCore::handle_retrieval_query` in
`crates/daemon/src/query_route.rs`, whose `before_phase` hook exposes each
budget check to a test. The host-level witnesses run a real host with a real
client in `crates/daemon/src/request_budget/host_tests.rs`; the route
witnesses run a `KernelDaemon` over a file-backed projection populated from
its kernel in `crates/daemon/tests/query_route.rs` and drive the handler in
`crates/daemon/tests/query_route_handler.rs`. A limit set is validated at
installation by `QueryRouteLimits::validate`, so a batch the kernel could never
judge is refused before any request instead of on every request.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [route-budget-is-derived-once-before-queue-wait](#route-budget-is-derived-once-before-queue-wait) | safety | test-only | always | active | high |
| [route-sql-cancellation-is-request-local](#route-sql-cancellation-is-request-local) | safety | test-only | always | active | high |
| [route-permits-and-pins-outlive-client-cancellation](#route-permits-and-pins-outlive-client-cancellation) | safety | test-only | always | active | medium |
| [route-authorization-precedes-materialization](#route-authorization-precedes-materialization) | safety | test-only | always | active | high |
| [route-bounds-are-enforced-before-protected-work](#route-bounds-are-enforced-before-protected-work) | safety | test-only | always | active | medium |
| [route-cancellation-is-observed-in-every-phase](#route-cancellation-is-observed-in-every-phase) | safety | test-only | always | active | high |
| [route-cancelled-work-drains-within-approved-envelope](#route-cancelled-work-drains-within-approved-envelope) | liveness | test-only | always | active | low |
| [route-candidate-ids-never-widen-scope](#route-candidate-ids-never-widen-scope) | safety | test-only | always | active | high |
| [route-final-revalidation-precedes-every-result](#route-final-revalidation-precedes-every-result) | safety | test-only | always | active | high |
| [route-healthy-authorized-query-completes-fused](#route-healthy-authorized-query-completes-fused) | liveness | test-only | always | active | medium |
| [route-required-context-failure-is-typed-and-terminal](#route-required-context-failure-is-typed-and-terminal) | safety | test-only | always | active | medium |
| [route-rollback-disables-without-mutating-canonical-truth](#route-rollback-disables-without-mutating-canonical-truth) | safety | test-only | always | active | high |

## Records

### route-budget-is-derived-once-before-queue-wait

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/src/request_budget.rs` unit tests
`the_remaining_duration_is_clamped_to_the_approved_ceiling`,
`a_request_without_an_approved_ceiling_or_a_remaining_duration_is_refused`,
`dropping_the_guard_cancels_every_clone_and_the_stop_predicate`; and
`crates/daemon/tests/request_budget_reads.rs`
`the_deadline_is_identical_in_the_guard_every_clone_and_the_blocking_thread`,
`an_elapsed_remaining_duration_interrupts_a_held_read_the_same_way`,
`a_cancelled_budget_leaves_the_connection_wait_before_the_holder_releases`.
Guarantee: One absolute budget is derived at handler entry from the request's
cancellation and its clamped `remaining_ms`; every later stage, including the
connection acquisition wait and SQLite VM steps, observes that same deadline
and flag, and no stage re-derives a relative budget.
Check: `always` - the guard, every `SharedBudget` clone, the `EvalBudget` a
callee borrows, and the stop predicate a blocking thread carries report one
identical deadline; a missing ceiling, a missing or zero `remaining_ms`, or an
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
Existing check: `crates/kernel/tests/kernel_source_budgets.rs` uses a
test-local cancel-on-drop budget, status unaudited; `crates/retrieval/tests/lexical_retrieval.rs`
`an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion`
drives the storage scope directly, status unaudited.
Impact: A stage with its own fresh budget could outlive the caller's deadline,
and a transport timeout could stand in for the caller's remaining duration.
Open questions: None.

### route-sql-cancellation-is-request-local

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/request_budget_reads.rs`
`a_later_request_on_the_same_connection_is_not_interrupted_by_a_prior_cancellation`,
`cancelling_the_request_interrupts_a_held_read_and_reports_exhaustion`; and
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
and the budget classifies it as cancellation; the negative control shows that
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
status unaudited; `crates/kernel/tests/budget_tests.rs`
`commit_clears_interrupt_before_sql_and_rearms_it_for_connection_reuse` for the
kernel store, status unaudited.
Impact: A leaked hook would fail an unrelated request with a spurious deadline
or leave an interrupted read reported as an engine failure.
Open questions: None.

### route-permits-and-pins-outlive-client-cancellation

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the bridge clauses are covered by
`crates/daemon/src/request_budget/host_tests.rs`
`cancelling_a_suspended_handler_interrupts_the_held_read_and_joins_it_before_settling`,
`the_guard_drop_alone_interrupts_the_held_read_when_the_host_aborts_the_handler`,
and `a_panic_in_tracked_blocking_work_is_typed_and_still_settles`; the permit
and pin clauses wait for the route (U3b) and the dense lane (U3c) that hold
them.
Guarantee: After client cancellation the blocking worker is joined before the
request settles; the connection it holds stays held until the read returns
and is released only after the join; a panic inside the blocking closure
surfaces as its typed failure and still settles.
Check: `always` - a real client cancels a request whose handler is suspended
at `run_blocking` with no `select!` arm; the read reports exhaustion at a time
no later than the host's error publication for that channel; the projection
connection is held before cancellation and free after settlement; with the
budget derived from a token the test never cancels, the guard's drop alone
stops the read and classifies it as cancellation; a panicking closure yields
`BlockingFailure::Panicked` and the client sees one terminal error. `always`
because every cancelled request must drain its blocking work.
Fault/timing angle: The host aborts the handler future while the blocking
read runs; in the drop-only variant the guard's drop is the only path raising
the interrupt.
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

- The route holds its `RequestBudget` guard across the `run_unit` join and the
  lifecycle pin inside the blocking closure; a holder census over those and
  the dense pins U3c adds is still to be written. (needs human input)

### route-authorization-precedes-materialization

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/query_route_handler.rs`
`scope_harness_and_disable_are_decided_before_any_candidate_read`.
Guarantee: A request naming another project root, an unbound session, or a
harness other than the route binding's is refused before the body is parsed,
before any candidate row is read, and before any payload byte is touched; the
binding is the only authority and no request field grants scope.
Check: `always` - on a daemon whose kernel holds rows, a foreign
`project_root` answers the kernel routes' `invalid`/`project_mismatch` state,
an unknown `session_id` answers a transport error, and a `harness` claim other
than the bound one answers the `unauthorized` terminal; a claim equal to the
bound harness and an absent claim both pass to the next stage. `always`
because every request crosses the same entry.
Fault/timing angle: None; the decision is made on the bound route state before
any blocking work is admitted.
Required faults and enabling state: A bound route with a known harness; a
second project root under the same parent; an installed limit set so the
harness check is the refusing stage.
Confidence: high - [evidence](evidence/route-authorization-precedes-materialization.md).
The route reuses `kernel_request`, whose scope check runs before body parse
and whose `RouteScope` carries the harness from the same binding read, so the
claim is compared against the binding the query then runs under, before the
limit set or the budget is read.
Existing check: `crates/daemon/tests/kernel_routes.rs` project-mismatch
assertions for `kernel.read` and `kernel.commit`, status unaudited.
Impact: A request could read another project's candidate rows or prose, or a
caller could name a harness to inherit its capability set.
Open questions: None.

### route-bounds-are-enforced-before-protected-work

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/query_route.rs`
`each_bound_saturates_before_its_protected_work` and
`crates/daemon/tests/query_route_handler.rs`
`scope_harness_and_disable_are_decided_before_any_candidate_read`.
Guarantee: Every bound in `QueryRouteLimits` is checked before the work it
protects: query bytes before classification, the selector count and lexical
atoms against `probes` before any page or scan, scan rows and accepted rows
inside the lexical lane, the fused union inside `fuse`, the validation batch
per kernel call, result rows and response bytes before each entry is
serialized; an absent limit set disables the route; a limit set the kernel
could not serve is refused at installation; the U3a bridge refuses an
unapproved deadline ceiling.
Check: `always` - `result_rows = 1` materializes one entry and reports
`truncated` while the fused ranking keeps every entry; a 200-byte
`response_bytes` materializes fewer entries than the ranking holds and the
serialized body is at most 200 bytes; `fused_union = 1` ends the request as
`lane_unavailable` with reason `fused_union` and the fusion phase is the last
one reached; one exact page of one row reports the exact lane `incomplete`
with reason `page_bound`; `validation_batch = 1` judges in several batches and
returns the same entries; `probes = 1` refuses two selectors as invalid before
the exact phase runs and reports two prose atoms as the lexical lane
`unavailable` while the exact lane still serves; `lexical_accepted = 1` and
`lexical_scan_rows = 1` report `accepted_bound` and `scan_bound`; a query over
`query_bytes` is refused as invalid at the handler and again by `classify`; a
`validation_batch` over the kernel's candidate maximum is refused by
`set_query_route_limits` and installs nothing. `always` because each bound must
hold on every request.
Fault/timing angle: None; saturation is reached by shrinking one limit at a
time on a fixed corpus.
Required faults and enabling state: A projection with more matching rows than
the shrunken bound; the `before_phase` hook to show which phase was not
reached.
Confidence: medium - [evidence](evidence/route-bounds-are-enforced-before-protected-work.md).
Every bound has a route-level witness; the approved values are not yet fixed.
Existing check: `crates/retrieval/tests/lexical_retrieval.rs` scan and accepted
bound tests, status unaudited; `crates/retrieval/tests/exact_lookup.rs` page
cursor tests, status unaudited; `crates/retrieval/tests/fusion.rs` union bound
test, status unaudited.
Impact: An unbounded phase could read or serialize past the approved envelope
under one request's budget.
Open questions:

- The approved values for every limit are an RP2.9 item; the tests use
  test-local values. (needs human input)

### route-cancellation-is-observed-in-every-phase

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/query_route.rs`
`cancellation_and_deadline_are_observed_in_every_phase` and
`a_healthy_query_completes_fused_in_the_oracles_order`.
Guarantee: The request budget is checked at the start of probe compilation,
the exact scan, the lexical scan, fusion, revalidation, materialization, and
response construction; a cancellation or a lapsed deadline observed at any of
them ends the request with `cancelled` or `deadline` and no ranking.
Check: `always` - for each of the seven phases, a hook that cancels the
budget's token when that phase begins yields `Terminal::Cancelled`, and a hook
that sleeps past a 600 ms remaining duration yields `Terminal::Deadline`; in
both halves the hook records the phases reached and the target is the last
one, so the terminal is attributed to that phase; a healthy run visits the
seven phases once each in order. The revalidation check guards the kernel
judging, and each validation batch checks again. `always` because every phase
transition performs the check.
Fault/timing angle: Cancellation between phases; the projection read's own
progress handler covers cancellation inside a statement, see
`route-sql-cancellation-is-request-local`.
Required faults and enabling state: A populated projection; the `before_phase`
hook; a `CancellationToken` behind `CancelSignal::observing`.
Confidence: high - [evidence](evidence/route-cancellation-is-observed-in-every-phase.md).
The phases are enumerated in `Phase` and each check is one call site.
Existing check: `crates/daemon/tests/request_budget_reads.rs` interruption
tests for a single read, status unaudited.
Impact: A phase without a check would run to completion under a dead budget
and could return a ranking the caller no longer waits for.
Open questions: None.

### route-cancelled-work-drains-within-approved-envelope

Type: liveness
Reachability: test-only
Status: active
Exercised: partial - the join-before-settle clause is covered by
`crates/daemon/src/request_budget/host_tests.rs`
`cancelling_a_suspended_handler_interrupts_the_held_read_and_joins_it_before_settling`;
the route's own holders and the envelope bound are not yet measured.
Guarantee: Once a request is cancelled, the tracked blocking work that runs
its lanes finishes, the lifecycle pin and the projection connection it holds
are released, and the request settles within an approved fault-free envelope;
a permanently blocked read stays visible as unresolved work rather than being
settled early.
Check: `always` - the handler awaits `run_unit` with the budget guard alive
and drops the guard only after the join; the closure owns the pin for its
whole run. The envelope bound and a holder census over the pin and the guard
are not yet asserted. `always` because every cancelled request must drain.
Fault/timing angle: Cancellation while the lanes hold the projection
connection; cancellation while the closure waits to pin the lifecycle.
Required faults and enabling state: A real host and client; a held projection
read; a pinned family; an approved drain envelope.
Confidence: low - [evidence](evidence/route-cancelled-work-drains-within-approved-envelope.md).
Only the ordering clause has a witness, and it runs on the bridge rather than
the route.
Existing check: `crates/host-runtime/tests/dispatch.rs`
`cancel_waits_for_the_request_blocking_work`, status unaudited.
Impact: A settle before the drain would release the connection or the pin while
a lane still uses it; an unbounded drain would hide a wedged read.
Open questions:

- The drain envelope is an RP2.9 number; until it is approved the record has
  no bound to assert. (needs human input)

### route-candidate-ids-never-widen-scope

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/query_route.rs`
`revalidation_excludes_retired_and_foreign_occurrences` and
`crates/daemon/tests/query_route_handler.rs`
`scope_harness_and_disable_are_decided_before_any_candidate_read`.
Guarantee: The request carries no occurrence identifiers, and the lexical
lane and revalidation judge every candidate under the route binding's project
scope; an occurrence a lane ranks is materialized only when the kernel judges
it eligible under that scope, so no identifier a caller or a lane produces
widens authorization.
Check: `always` - a request body with an `occurrence_ids` field is refused
as invalid by `deny_unknown_fields`; under a foreign project scope the same
projection rows yield a `fused` answer with no entries because every lane
result is excluded before materialization. `always` because every entry passes
revalidation.
Fault/timing angle: None.
Required faults and enabling state: A projection populated from one project;
a second `ProjectScope` that owns none of its rows.
Confidence: high - [evidence](evidence/route-candidate-ids-never-widen-scope.md).
The scope is taken from `RouteScope`, never from the body.
Existing check: `crates/daemon/tests/claim_eligibility.rs`
`retrieval_adapter_agrees_with_daemon_and_kernel_on_one_snapshot` foreign-scope
assertions, status unaudited.
Impact: A caller could name an identifier from another project and receive
its position or payload.
Open questions: None.

### route-final-revalidation-precedes-every-result

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/query_route.rs`
`revalidation_excludes_retired_and_foreign_occurrences`.
Guarantee: Every fused entry is judged by the kernel's budget-aware
eligibility adapter under the bound scope and destination after fusion and
before materialization; an entry the kernel no longer admits is filtered out
without recomputing scores or positions; no retrieval-side verdict copy is
consulted.
Check: `always` - after a decision is retired in the kernel while the
projection still holds its rows, the same query returns strictly fewer
entries, every remaining entry was present before, and the answer is not
degraded; under a foreign scope every entry is excluded. `always` because the
filter runs on every result set.
Fault/timing angle: The retirement lands between the projection snapshot and
the query.
Required faults and enabling state: A projection built from a kernel snapshot;
a later `retire_decision` commit materialized into the kernel's source
descriptors.
Confidence: high - [evidence](evidence/route-final-revalidation-precedes-every-result.md).
`live_candidates_by_id` reads the candidates the lanes produced at the lanes'
snapshot and `judge_occurrences_within_budget` judges them in
`validation_batch` slices; `Fused::filter` keeps positions, so a withheld
entry leaves a gap in `position`, which the specification accepts because
positions are never recomputed after fusion.
Existing check: `crates/daemon/tests/claim_eligibility.rs` retirement
assertions, status unaudited; `crates/retrieval/tests/fusion.rs` filter test,
status unaudited.
Impact: A retired or corrected occurrence could be returned with a live
position.
Open questions: None.

### route-healthy-authorized-query-completes-fused

Type: liveness
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/query_route.rs`
`a_healthy_query_completes_fused_in_the_oracles_order` and
`a_single_declared_lane_serves_and_no_lane_is_refused`;
`crates/daemon/tests/query_route_handler.rs`
`the_running_daemon_serves_the_route_from_its_converged_family`.
Guarantee: An authorized query over healthy lanes with a sufficient budget
returns a nonempty `fused` answer whose entry order equals the U2 fusion of
the lanes' own rankings, with both declared lanes `complete`, the dense lane
`undeclared`, and raw scores retained per lane.
Check: `always` - over a projection populated from the daemon's kernel, the
route's entry order equals a hand-summed weighted RRF (`weight / (k +
position)` over the lanes' positions, unequal weights, `k = 7`, ties by
identifier bytes) computed in the test from rankings it builds directly with
`exact::page` and `lexical::retrieve`, so the oracle shares no code with
`fuse`; at least one entry carries an exact
contribution and one a lexical contribution; a selector-only query serves the
exact lane alone, a prose-only query the lexical lane alone, and a query that
declares no lane is refused as invalid. Through the handler, a daemon whose
lifecycle converged a family answers `fused` with both lanes `complete`.
`always` because every healthy request must complete.
Fault/timing angle: None.
Required faults and enabling state: A populated projection; an installed
limit set; a converged family for the handler path.
Confidence: medium - [evidence](evidence/route-healthy-authorized-query-completes-fused.md).
The handler-level witness serves an empty family: committing decisions after
convergence left the family behind the kernel's freshness limit in this
harness, so the nonempty oracle comparison is made at the `execute` level.
Existing check: `crates/retrieval/tests/fusion.rs` oracle tests, status
unaudited.
Impact: An always-refusing route would pass every safety record and serve
nothing.
Open questions:

- A handler-level nonempty answer needs a harness that catches the family up
  to decisions committed after convergence. (needs human input)

### route-required-context-failure-is-typed-and-terminal

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `crates/daemon/src/query_route.rs`
`every_terminal_has_one_wire_code_and_the_response_names_it` fixes the closed
set and its codes; `crates/daemon/tests/query_route.rs`
`a_lane_that_cannot_run_degrades_the_answer_while_the_other_serves` drives a
failed exact lane (`no_checkpoint`) through the route with the lexical lane
serving; no route stage raises `RequiredContextFailure` until the packing
integration lands.
Guarantee: The route's terminal set is closed - `unauthorized`, `deadline`,
`cancelled`, `lane_unavailable`, `required_context_failure`, `disabled` - each
with one wire code carried in a `terminal` response, and a required-context
failure raised by packing is one of them rather than a degraded answer.
Check: `always` - every variant maps to a distinct code and to a
`{"kind":"terminal","terminal":<code>}` response; a `lane_unavailable`
terminal carries a `reason` code; the failure of a non-dense lane for a
non-budget reason is reported per lane as `unavailable` with a closed reason
code and the answer `degraded` while the other lane's ranking is served, and
both lanes failing is `lane_unavailable`.
`always` because the set is the wire contract.
Fault/timing angle: None.
Required faults and enabling state: A packing stage that misses required
context, which U4 supplies.
Confidence: medium - [evidence](evidence/route-required-context-failure-is-typed-and-terminal.md).
The variant exists and is serialized; its producer does not.
Existing check: `crates/daemon/src/kernel_routes/state.rs` closed
`KernelOutcome` set, status unaudited.
Impact: A stringly or open terminal set would let a new failure reach the
harness untyped.
Open questions: None.

### route-rollback-disables-without-mutating-canonical-truth

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/query_route_handler.rs`
`scope_harness_and_disable_are_decided_before_any_candidate_read` and
`the_running_daemon_answers_a_fused_query_from_its_converged_family`.
Guarantee: The route is disabled by removing its limit set; while disabled
every authorized request receives the `disabled` terminal and no kernel,
projection, or lifecycle state is read or written by the route; re-enabling
installs a limit set and requires no data change.
Check: `always` - a fresh daemon answers `disabled`; a refused limit set
installs nothing; after limits are installed it serves; after
`set_query_route_limits(None)` it answers `disabled` again and the kernel's
tip and lease epoch are unchanged; after reinstalling it serves an answer
equal to the one before the disable and the kernel is still unchanged.
`always` because the check is the first stage after authorization.
Fault/timing angle: None.
Required faults and enabling state: A converged family so the enabled answer
is `fused`.
Confidence: high - [evidence](evidence/route-rollback-disables-without-mutating-canonical-truth.md).
The disable path returns before the budget is derived or the pin is taken.
Existing check: `crates/daemon/tests/search_replacement/disable.rs` family
disable tests, status unaudited.
Impact: A rollback that mutated canonical rows to match a projection would be
irreversible.
Open questions: None.
