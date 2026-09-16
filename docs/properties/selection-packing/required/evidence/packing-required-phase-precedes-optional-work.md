# packing-required-phase-precedes-optional-work

## Discovery trigger

RP2.8's packing contract requires every required occurrence to be validated
and reserved before any optional work, with exactly one `PreparationFailure`
class when it cannot be, zero optional events, and no new retrieval.
Acceptance row AC3 names the six fault classes.

## Evidence trail

- `crates/retrieval/src/packing/required.rs` `RequiredContextFailure` holds
  the classes; `admit_required` maps rows and dispositions to them in request
  order before any byte is loaded; `reserve_required` adds `Corrupt` for a
  length mismatch and `OverBudget` for the limit.
- `crates/daemon/src/packing.rs` `prepare_required` refuses a request set beyond the load
  bound before any read, reads each row under one connection hold, judges
  eligibility in one kernel batch, admits, loads through `load_payload` under
  a second hold, and reserves outside both, polling `EvalBudget::is_exhausted`
  between stages and recording only `StageEvent::Required` events. It never
  calls a retrieval lane; the test seeds the trace with one lane call and
  observes the count unchanged.
- `crates/daemon/tests/packing_required.rs` constructs each fault and asserts
  the class, its literal, `optional_events() == 0`, and
  `retrieval_calls() == 0`.
- `crates/retrieval/tests/packing_required.rs` covers the verdict mapping
  the kernel fixture cannot produce (`Hidden`, `Stale`, `Superseded`).

## Failure scenario

An optional item admitted before the required set is reserved consumes budget
the required set needed, so the required phase fails after optional bytes are
already rendered, or a required occurrence that should have refused is
rendered stale or corrupt.

## Timing windows and dependencies

The `EvalBudget` deadline is polled before the read, before the kernel batch,
and before the loads; the kernel batch itself runs within the budget through
`judge_occurrences_within_budget`.

## What a test must construct

- A seeded kernel whose objects are admitted, so a live row judges `Ok`.
- One fixture per fault class, including a raw-connection corruption and a
  tombstone.
- An expired and a cancelled `EvalBudget`.
- A `PackingTrace` inspected after every outcome.
