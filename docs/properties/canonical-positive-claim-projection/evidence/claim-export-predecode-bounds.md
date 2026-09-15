# claim-export-predecode-bounds

## Discovery trigger

Specification 'Projection, progress and recovery': stable paging and row/byte bounds apply before decode and materialization, not caller truncation after an unbounded load.

## Evidence trail

- `crates/kernel/src/claim_causality.rs`: `causal_class_at` selects `length(observation_payload)` first and returns `Unknown(Oversized)` without reading the payload when it exceeds `max_payload_bytes`.
- `crates/kernel/src/source_export.rs`: `admit` charges row, encoded, and decoded bounds before any descriptor object is read.
- `crates/kernel/tests/kernel_claim_facts.rs`: `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly` sets an 8-byte bound and asserts `Oversized` with no record summary.

## Failure scenario

Decoding before bounding lets one oversized detail allocate its full size in every reader that names the subject.

## Timing windows and dependencies

None.

## What a test must construct

Write a real record, read with a bound smaller than its payload, assert `Unknown(Oversized)` and `causal_record == None`.

## Investigation log

No open questions at authoring time.
