# uo-p-record-survives-session-delete

## Discovery trigger

Spec #1145 KTD1; activated here with its Kernel-side check (deviation confirmed
at #1154 ticketing).

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1155 branch tip, the merge of `6d15ee29c` with
`main` at `d3b772026`.

- Delete triggers: `crates/kernel/src/schema.rs:188`.
- Retained read: `served_set` (`crates/kernel/src/use_outcome.rs:393`) answers
  retired records.

- Guarantee under test: After `session.delete` answers `ok`, the session's
  serving records and decision rows are readable, equal in count and ids to
  before.
- Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2177`);
  `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
  (`crates/kernel/tests/kernel_use_outcome.rs:400`) (a retired record stays
  readable). Status: unaudited.

## Failure scenario

A delete path that removed Kernel rows would erase the served set.

## Timing windows and dependencies

None.

## What a test must construct

#1156 adds the `session.delete` route test.

Required faults and enabling state: A session with records, then a delete
(Kernel arm: direct SQL deletes).

## Investigation log

### Q: The `session.delete` arm lands with #1156, which extends `Exercised`.

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The `session.delete` arm lands with #1156, which extends
  `Exercised`.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
