# packing-attribution-follows-the-occurrence-row

## Discovery trigger

RP2.8 KTD1 keeps payload identity (collision-checked bytes) and occurrence
identity (logical retrieval identity) separate, and the identity contract says
attribution follows the occurrence row, never a lookup keyed by a payload
identifier from another occurrence. Acceptance row AC1 names the byte-twin
read-back and the collision refusal.

## Evidence trail

- `crates/retrieval/baseline.sql` `payloads` is keyed by the digest of exact
  bytes and `occurrences` carries class, tuple, revision, representation, span,
  payload reference, sensitivity, and source columns per occurrence.
- `crates/retrieval/src/lib.rs` `persist_with_digests` refuses
  `PayloadCollision` and `OccurrenceCollision` before the first insert.
- `crates/retrieval/src/packing.rs` `read_selected` queries by occurrence
  identifier, refuses a row whose tuple does not digest to that identifier,
  and returns `SelectedOccurrence` values whose `payload` is a `PayloadRef`
  exposing identifier and byte length only; no function in the module takes a
  `PayloadRef` as an input.
- `crates/retrieval/src/packing.rs` `SelectedOccurrence::eligibility_candidate`
  builds the kernel candidate from the row's own source columns; the verdict
  is returned by `crates/retrieval/src/eligibility.rs` `judge_occurrences` and
  stored nowhere in the crate.
- `crates/retrieval/tests/packing_identity.rs`
  `byte_twins_share_one_payload_row_and_keep_their_own_attribution` asserts the
  counts, the per-row attribution, the payload-keyed collapse, and the kernel
  report order.

## Failure scenario

A packer that deduplicates by payload identifier renders one twin and applies
its sensitivity and eligibility to both sources; a `Secret` span from one tool
call would leave the host under a `Normal` twin's verdict, or a live span
would be hidden under a retracted twin's.

## Timing windows and dependencies

None. The read is a pure function of the stored rows; eligibility is
snapshot-bound in the kernel report and never cached in retrieval.

## What a test must construct

- Two raw tool spans over one buffer with distinct tool calls, sensitivities,
  and sources, persisted together.
- A read over both identities, then a map keyed by payload reference.
- A kernel store and project scope to judge the returned candidates.
- The `test-support` digest seam presenting other bytes under a stored digest.
