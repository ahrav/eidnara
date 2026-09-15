# claim-format-rollback-preserves-canonical-state

## Discovery trigger

Specification 'Failure and rollback': no rollback mutates canonical truth to match projection; KTD2 makes canonical format changes explicit epoch decisions.

## Evidence trail

- `crates/kernel/src/claim_causality.rs`: `decode_detail` reads `causality_version` before the rest and maps a mismatch to `UnknownReason::UnsupportedVersion`.
- `crates/kernel/tests/kernel_claim_causality.rs`: the version branch of `replay_is_effect_free_and_conflicting_or_unsupported_records_are_unknown` rewrites the stored version to 2 and asserts `Unknown(UnsupportedVersion)`.

## Failure scenario

A reader that decoded a newer layout by field name could misread a renamed field as evidence and grant a class the newer writer never asserted.

## Timing windows and dependencies

Newer binary writes, older binary reads after a rollback.

## What a test must construct

Store a record, rewrite its version out of band, read at the same snapshot, assert `Unknown` and that the row bytes are unchanged.

## Investigation log

### Q: What is the approved epoch/rebuild path for a canonical schema change?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: needs human input
