# dense-one-request-budget-spans-every-stage

## Discovery trigger

RP2.6 resources and #620 AC1: one cloned `EvalBudget` carries the original
deadline and request cancellation through embedding, SQLite progress, the
compressed scan, and the original reads.

## Evidence trail

- `handle_retrieval_query` derives the `RequestBudget` from the runner's
  cancel signal before the embedding wait and passes `shared.eval()` to the
  unit.
- The unit runs `execute`, whose projection read is `read_under` with the
  budget's stop predicate; the walk checks the budget at every row and batch;
  `rank_compressed` checks it before every original read, on both sides of
  the read's observer (`crates/daemon/src/vector_reader.rs:902`, `:911`), so a
  cancellation delivered at `ReadOriginal` ends the request before the row's
  bytes are read.
- The kernel judges each batch with `judge_eligibility_within_budget`.

## Failure scenario

A stage with a fresh relative budget keeps running after the caller
cancelled, or past the deadline.

## Timing windows and dependencies

Inside each stage, through a real host's cancellation.

## What a test must construct

- Cancellation at the first visited row, after a judgment, and at an original
  read; a deadline lapse after selection; a client cancellation through the
  host.

## Investigation log

### Q: Does a retry or queue wait refresh the deadline?

- Sources examined: `RequestBudget::derive`, the route.
- Findings: the deadline is fixed at derivation; nothing re-derives it.
- Missing evidence: none.
- Conclusion: resolved with answer - no.
