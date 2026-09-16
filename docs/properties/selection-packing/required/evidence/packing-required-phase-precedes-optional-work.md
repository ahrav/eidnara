# packing-required-phase-precedes-optional-work

## Discovery trigger

RP2.8's packing contract requires every required occurrence to be validated
and reserved before any optional work, with exactly one `PreparationFailure`
class when it cannot be, zero optional events, and no new retrieval.
Acceptance row AC3 names the six fault classes.

## Evidence trail

- `crates/retrieval/src/packing/required.rs` `RequiredContextFailure` holds
  the classes; `admit_required` refuses a set beyond the load-count bound
  before examining any request, then maps rows and dispositions to the classes
  in request order before any byte is loaded; `reserve_required` adds `Corrupt`
  for a length mismatch and `OverBudget` for the limit.
- `crates/daemon/src/packing.rs` `prepare_required` tightens the load bound to
  the kernel's `MAX_ELIGIBILITY_CANDIDATES`, refuses a request set beyond it
  before any read, reads each row under one connection hold, judges
  eligibility in one kernel batch, admits, fetches payloads through
  `fetch_payload` under a second hold, verifies each digest through
  `PayloadRef::verify` after that hold is released, and reserves outside both.
  Each hold goes through `with_conn_interruptible` under the budget's deadline
  and cancellation when the budget has a deadline, stops at the first faulting
  statement, and maps `StoreError::Deadline` to the deadline refusal;
  `EvalBudget::is_exhausted` is polled between stages and only
  `StageEvent::Required` events are recorded. It never calls a retrieval lane;
  the test seeds the trace with one lane call and observes the count
  unchanged.
- `crates/daemon/tests/packing_required.rs` constructs each fault and asserts
  the class, its literal, `optional_events() == 0`, and
  `retrieval_calls() == 0`; two tests hold the projection connection on
  another thread and assert the phase refuses within the budget with no event.
- `crates/retrieval/tests/packing_required.rs` covers the verdict mapping
  the kernel fixture cannot produce (`Hidden`, `Stale`, `Superseded`) and the
  load-bound precedence over a per-request fault.

## Failure scenario

An optional item admitted before the required set is reserved consumes budget
the required set needed, so the required phase fails after optional bytes are
already rendered, or a required occurrence that should have refused is
rendered stale or corrupt.

## Timing windows and dependencies

The `EvalBudget` deadline is polled before the read, before the kernel batch,
and before the loads; the kernel batch itself runs within the budget through
`judge_occurrences_within_budget`. Each projection hold acquires the
connection by polling until the budget's deadline and installs the budget as
the SQLite progress stop, so a holder on another thread or a long statement
cannot carry the phase past its deadline or its cancellation; a budget with no
deadline is bounded only by the polls between stages.

## What a test must construct

- A seeded kernel whose objects are admitted, so a live row judges `Ok`.
- One fixture per fault class, including a raw-connection corruption and a
  tombstone.
- An expired and a cancelled `EvalBudget`.
- A `PackingTrace` inspected after every outcome.
