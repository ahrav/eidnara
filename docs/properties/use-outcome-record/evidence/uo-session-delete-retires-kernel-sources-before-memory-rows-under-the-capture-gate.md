# uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate

## Discovery trigger

Spec #1145 C10 and D5.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `1c56ba033`).

- Handler and step order: `handle_session_delete_value`
  (`crates/daemon/src/lib.rs:6881`).
- Test hook `stop_after_retirement`: `crates/daemon/src/lib.rs:6917`.
- Correction: the spec's `lib.rs:25158, 25911` were test functions that moved by
  31 lines.

- Guarantee under test: `session.delete` holds the capture commit gate, retires
  the session's Kernel sources in pages, then deletes the Memory Store rows; a
  Kernel failure returns before the Memory Store is touched.
- Existing check:
  `session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`
  (`crates/daemon/src/source_capture_tests.rs:1547`);
  `session_delete_with_the_kernel_starting_deletes_nothing`
  (`crates/daemon/src/source_capture_tests.rs:1617`);
  `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages`
  (`crates/daemon/src/source_capture_tests.rs:1689`);
  `session_delete_waits_for_the_capture_gate_off_the_runtime_worker`
  (`crates/daemon/src/lib.rs:25942`). Status: unaudited.

## Failure scenario

A memory delete before the Kernel step would leave a later retry with no session
to mark.

## Timing windows and dependencies

Between the Kernel step and the memory delete.

## What a test must construct

The existing stop-between-stores test.

Required faults and enabling state: A session with captured sources; the
`stop_after_retirement` hook.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
