# uo-per-session-pass-timestamps-die-with-the-session

## Discovery trigger

Spec #1145 K6.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `d5ebb3598`).

- Writes happen only when state changed: `crates/daemon/src/transform.rs:3257`,
  `crates/daemon/src/transform.rs:3756`, `crates/daemon/src/transform.rs:5379`.

- Guarantee under test: The per-session pass timestamp
  `last_committed_pass_at_ms` lives on Memory Store session meta and is deleted
  with the session.
- Existing check:
  `delete_session_sweeps_every_session_table_once_and_scopes_notes`
  (`crates/memory-store/src/lib.rs:22343`). Status: unaudited.

## Failure scenario

A surviving timestamp would contradict the delete.

## Timing windows and dependencies

None.

## What a test must construct

The existing test.

Required faults and enabling state: A session with committed passes, then
`session.delete`.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
