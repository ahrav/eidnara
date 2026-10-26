# uo-p-served-ids-equal-frozen-composition

## Discovery trigger

Spec #1145 C4.

Record kind: proposed obligation; its owning ticket #1155 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `8ace6c9f8`).

- Digest: `served_set_digest` (`crates/kernel/src/use_outcome.rs:56`).

- Guarantee under test: The stored decision set equals, as a set, the object ids
  of the snapshot rows whose composition the same fold persisted.
- Existing check: Kernel half only:
  `duplicate_and_malformed_serves_are_refused_without_rows`
  (`crates/kernel/tests/kernel_use_outcome.rs:215`);
  `session_roles_round_trip_and_the_served_set_digest_is_order_free`
  (`crates/kernel/tests/kernel_use_outcome.rs:752`). The writer is proposed.
  Status: unaudited.

## Failure scenario

Ids drawn from a later read.

## Timing windows and dependencies

None.

## What a test must construct

#1155.

Required faults and enabling state: A read trimmed by the token budget; a CAS
retry.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the Kernel half cited above agrees with the guarantee at this tip;
  the writer is proposed and no code at this tip establishes its behavior.
- Missing evidence: the writer path, which lands with the owning ticket.
- Conclusion: unresolved, needs the writer; the Kernel half is settled.
