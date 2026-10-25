# uo-p-producer-matches-request-subagent-flag

## Discovery trigger

Spec #1145 C16.

Record kind: proposed obligation; its owning ticket #1155 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `1c56ba033`).

- Role: `crates/kernel/src/use_outcome.rs:29`.

- Guarantee under test: The stored `session_role` is `subagent` exactly when the
  writing pass's effective subagent flag was true.
- Existing check: none; proposed.

## Failure scenario

A role from request text rather than the pass flag.

## Timing windows and dependencies

None.

## What a test must construct

#1155.

Required faults and enabling state: A subagent request on the additive engine
with a Canonical read; a primary on each engine.

## Investigation log

### Q: OQ3-9 resolved at #1154 ticketing

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-9 resolved at #1154 ticketing: the column is `session_role`.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
