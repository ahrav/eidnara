# durable-private-result-recovers-without-model-refire

## Discovery trigger

Specification constraint: same-generation recovery with a valid claim adopts an
admissible durable result without a model call even after losing the
process-local broker, aliases, and transcript. Decision record Q15: the alias
table is not persisted. Open question 13.

## Evidence trail

`crates/kernel/src/review_staging.rs` - `ReviewDependencies` in the witness;
`ReviewStagingSpec.dependencies` required for proposal payloads;
`ReviewStagedRow.dependencies`; `sealed_review_reference`.

`crates/context-core/src/memory_reviewer_policy_union.rs` - `PolicyUnion::decode`
accepts only bytes that re-encode to themselves and their digest.

`crates/daemon/src/memory_reviewer/dependencies.rs` - `record` joins the
broker's union to the last completed marker; `revalidate` rebuilds each member's
expectation and runs `revalidate_under` on a fresh broker.

`crates/daemon/src/memory_reviewer/settlement.rs` - `adopt`.

`crates/daemon/src/memory_reviewer/coordinator.rs` - `prepare` skips subject
resolution when resuming; `investigate` returns from `adopt` before `open`.

Tests named in the catalog record.

## Failure scenario

A worker restarts after the Kernel envelope committed. It rebuilds a broker,
re-reads the subject, sends a new request, and stages a second result at the
same identity, or abandons the sealed one.

## Timing windows and dependencies

Process loss after `stage_review_input` and `transfer_execution_to_review`,
before `complete_memory_reviewer_receipt`; process loss after the marker's
`Complete` terminal, before staging.

## What a test must construct

A sealed row with its record and a completed marker; a fresh broker with no
aliases under the transferred review hold, or under a newly acquired execution
hold covering nothing after the lost run's hold was released; `adopt`; a second `adopt` (fenced); a retired
member; a record whose union bytes were edited in place; a record removed with
`json_remove`; a completed marker with no row. Assert the published reference,
the read, the attempt count, and each refusal.

## Investigation log

### Q: Where can the union live without changing protocol 3?

- Sources examined: `review_staging.rs` payload encoding and witness storage;
  `docs/host-wire-protocol.md` review operations.
- Findings: the payload digest covers `ReviewPayload` only; the witness is a
  separate column byte-compared on replay; `review.read` emits the payload and
  never the witness.
- Missing evidence: none.
- Conclusion: resolved with answer; the witness carries the record.

### Q: What hold does adoption run under before the transfer?

- Sources examined: `coordinator.rs` unsettled-exit release; `acquire_hold`
  returning the live hold of the same binding; `extend_execution_hold`.
- Findings: the lost run's execution hold is live after a crash and returned to
  the resumed run with its coverage; after an unsettled exit it was released,
  and the resumed run's acquisition mints one covering nothing. Adoption
  extends whichever it holds over the payload's disclosed inputs before
  revalidating, so coverage is judged as the live run's read judged it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which marker is the proposal's?

- Sources examined: `disclosure.rs` terminal write order; `coordinator.rs`
  propose handling.
- Findings: the attempt's `Complete` terminal is written when the response is
  accepted, before the step is parsed, so at settlement the last completed
  marker at the generation is the one whose response the proposal is. The
  record stores the marker's own body and union digests and the broker's union
  digest; in production the two union digests agree.
- Missing evidence: none.
- Conclusion: resolved with answer.
