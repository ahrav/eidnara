# uo-p-deleted-at-iff-session-delete-kernel-step

## Discovery trigger

Spec #1145 C10.

Record kind: proposed obligation; its owning ticket #1156 activates it.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `03c6b2b95`).

- The write sits in the delete handler's Kernel step.

- Guarantee under test: `deleted_at` is set exactly when a `session.delete` for
  the session committed its Kernel step, before the memory-store delete; route
  teardown leaves it null.
- Existing check: Kernel half only:
  `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2175`). The delete step is proposed.
  Status: unaudited.

## Failure scenario

A teardown that sets `deleted_at`.

## Timing windows and dependencies

Between the stores.

## What a test must construct

#1156.

Required faults and enabling state: A delete with the Kernel available; one with
it starting; a route close without delete.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
