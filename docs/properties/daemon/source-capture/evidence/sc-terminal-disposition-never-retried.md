# sc-terminal-disposition-never-retried

## Discovery trigger

Part B obligation from #1066 comment 2, decisions D2 and D13, requirement R4,
constraint C6. An OpenCode user message edited in place keeps its
`time.created`, so its revision is unchanged. The U2 ticket (#1082) resolves
OQ5: changed bytes under one identity are a terminal `IdentityReused`, never a
minted revision.

## Evidence trail

`crates/daemon/src/harness_sources.rs`:

- `opencode_units` derives a user message's revision from `time.created`
  (assistant messages from `time.completed`).
- `PublishError::is_permanent` marks `IdentityReused`, `Descriptor`, and the
  listed `Artifact` kinds permanent, and `RevisionAhead`,
  `UnadmittedSource`, and `Kernel` retryable.
- `BatchRefusal::conflicting_unit` names the unit whose key holds a receipt
  under another request digest; `publish_screened` returns it as an error and
  commits nothing.

`crates/daemon/src/source_capture.rs`:

- `publish_locked`, error arm: the conflicting unit is recorded
  `Disposition::Terminal` in the memo and counted
  `units_identity_conflict`; its siblings are not recorded and are offered
  again on the next pass.
- `classify` maps `UnitTooLarge`, `IdentityReused`, permanent refusals, and
  `Superseded` to `Disposition::Terminal`.
- `CaptureJob::publish`, candidate selection: a memo entry with an equal
  digest and a non-pending disposition is skipped and counted
  `units_memo_skipped`. A terminal entry is never a candidate, so it is never
  overwritten with `Pending` while it lives.
- The memo empties on restart, on `SourceCapture::invalidate_sessions` (every
  `session.delete`), and on a poisoned lock, and evicts the oldest entries
  past `MEMO_BUDGET_BYTES` (4 MiB).

No code mints a revision from `observed_at` after a conflict.

## Failure scenario

The supervisor retries a terminal unit on every pass, amplifying writes on the
Kernel writer, or mints a revision that evades `IdentityReused` and replaces
the original bytes.

## Timing windows and dependencies

After a memo loss the next pass offers the terminal unit again. The Kernel
answers `Conflict` from the stored receipt before anything commits (a probe,
not a publication); that batch fails, the unit is terminal again, and its
siblings wait one pass. The prior live descriptor stays live throughout.

## What a test must construct

Publish an OpenCode user message, then send the same identity and
`time.created` with changed text twice. Assert one conflict, no tip change on
the third pass, and one live descriptor holding the original bytes. A legal
witness: a second pass with an unchanged window replays.

Landed checks:

- `crates/daemon/src/source_capture_tests.rs`
  `changed_bytes_under_one_identity_are_terminal_and_never_retried`
  constructs the above and asserts `units_identity_conflict == 1` after both
  edited passes, an unchanged tip, and one live row reading `original`.
- `a_concurrent_publication_of_a_unit_replays_or_conflicts` (F5) asserts the
  memo, not a second Kernel conflict, answers for the terminal unit
  (`units_memo_skipped == 1`).
- `crates/daemon/tests/harness_sources.rs`
  `capture_batch_names_the_unit_whose_receipt_conflicts` asserts the conflict
  names its unit and commits nothing.

## Investigation log

### Q: Daemon-minted revision for proven edits (OQ5)?

- Sources examined: `ticket-1082.md` (Resolved OQ5), `publish_locked`.
- Findings: U2 resolves OQ5 as D2: changed bytes are terminal
  `IdentityReused`, with no minted revision. The code has no minting path.
- Missing evidence: none.
- Conclusion: resolved with answer, in U2.

### Q: Is the re-probe after memo loss exercised?

- Sources examined: `source_capture_tests.rs`.
- Findings: no test forgets the memo, or deletes a session, after a terminal
  disposition and observes the second `Conflict` and the tip.
- Missing evidence: a test pairing `forget_memo_for_test` with a terminal
  unit, the `replay_after_memo_loss` precondition for this record.
- Conclusion: unresolved, needs that test.

### Q: Is `ReAdmissionBlocked` terminal (OQ20)?

- Sources examined: `ticket-1082.md`, `is_permanent`.
- Findings: U2 resolves OQ20 as terminal for the process lifetime; an empty
  memo after restart re-probes.
- Missing evidence: none.
- Conclusion: resolved with answer, in U2.
