# packing-required-phase-precedes-optional-work

## Discovery trigger

RP2.8's packing contract requires every required occurrence to be validated
and reserved before any optional work, with exactly one `PreparationFailure`
class when it cannot be, zero optional events, and no new retrieval.
Acceptance row AC3 names six of the seven fault classes; `OverBudget` is
the seventh, its own variant by the Q3 ruling in `../catalog.md`.

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
  `PayloadRef::verify` after that hold is released, and reserves outside both
  with a budget poll on each side of the reservation.
  Each hold goes through `with_conn_interruptible` under the budget's deadline
  and cancellation when the budget has a deadline, stops at the first faulting
  statement, and maps `StoreError::Deadline` to the deadline refusal;
  `EvalBudget::is_exhausted` is polled between stages and only
  `StageEvent::Required` events are recorded. It never calls a retrieval lane;
  the test seeds the trace with one lane call and observes the count
  unchanged.
- `crates/daemon/tests/packing_required.rs` constructs each fault and asserts
  the class, its literal, `optional_events() == 0`, and `retrieval_calls()`
  still equal to the one lane call the fixture seeds before the phase; two
  tests hold the projection connection on
  another thread and assert the phase refuses within the budget with no event;
  one cancels the budget from inside the estimator and asserts the deadline
  refusal instead of a materialization.
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
before the loads, and on both sides of the reservation; the kernel batch
itself runs within the budget through
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

## Investigation log

### Q: Is a payload whose digest fails a load or a refusal?

- Sources examined: the Q3 ruling in `../catalog.md`; `prepare_required`'s
  verification loop; review threads
  [#668 r4030909349](https://github.com/ahrav/eidnara/pull/668#discussion_r4030909349)
  and
  [#668 r4031290395](https://github.com/ahrav/eidnara/pull/668#discussion_r4031290395).
- Findings: the ruling names the projection read as the load. The first cut
  counted only verified payloads, so a corrupt row that returned bytes left
  `payload_loads()` at zero.
- Missing evidence: none.
- Conclusion: resolved with answer - the count and the `Loaded` event are
  recorded for every payload that came back, before any digest check.

### Q: Can a budget without a deadline stop a projection hold?

- Sources examined: `crates/daemon/src/packing.rs` `hold`;
  `crates/storage/src/lib.rs` `with_conn_interruptible`, which takes an
  `Instant`; `crates/daemon/src/request_budget.rs` `SharedBudget::deadline`,
  which returns an `Instant`; review thread
  [#668 r4030909327](https://github.com/ahrav/eidnara/pull/668#discussion_r4030909327).
- Findings: storage has no deadline-free interruptible mode, and no production
  budget reaches the phase without a deadline; `EvalBudget::unbounded()` is
  test-only. The polls between stages and around reservation are the only
  bound for such a budget.
- Missing evidence: a production caller with a deadline-free budget, which
  does not exist at this base.
- Conclusion: resolved with answer - documented ceiling, no storage change
  until a caller needs it.
