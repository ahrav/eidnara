# uo-p-producer-matches-request-subagent-flag

## Discovery trigger

Spec #1145 C16.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `03c6b2b95`).

- Role: `crates/daemon/src/use_outcome_writer.rs:126`.

- Guarantee under test: The stored `session_role` is `subagent` exactly when the
  writing pass's effective subagent flag was true.
- Existing check:
  `the_session_role_follows_the_pass_subagent_flag_on_each_engine`
  (`crates/daemon/src/use_outcome_writer_tests.rs:341`). Status: unaudited.

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
