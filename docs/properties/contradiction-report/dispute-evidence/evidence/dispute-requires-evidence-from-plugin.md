# dispute-requires-evidence-from-plugin

## Discovery trigger

Plan P4 rule A1 and the #1160 ticket: a `mark_disputed` or `contradict`
filed through the plugin route must cite evidence a later reader can
check. At the specification revision (`3a3b916b7`) the disposition
operation had no evidence field and the route hard-coded `evidence_id:
None`. PR A adds the fields and the requirement.

## Evidence trail

- The operation carries two optional fields, `evidence_id` and
  `evidence_ref`, under `deny_unknown_fields`
  (`crates/daemon/src/kernel_routes/commit.rs:137`, `:155-172`). The
  `reason` field is `#[serde(skip)]` (`:168-171`), so a body that sends
  `reason` is refused as an unknown field and the caller cannot supply it.
  `a_disposition_carrying_its_own_reason_is_refused_as_an_unknown_field`
  (`crates/daemon/tests/kernel_routes.rs:6258`) sends such a body and
  asserts an error with the four row counts unchanged; it asserts the
  error variant, not the refusal name.
- `compose_reason` (`commit.rs:208-236`) routes dispute events to
  `DisputeReason::validated` and refuses evidence on every other event
  with `InvalidReason::InvalidInput` (`:228-229`); non-dispute events get
  `ADMISSION_REASON` (`:231`, constant at `:100`).
- `DisputeReason::validated` refuses a dispute with neither field and an
  empty `evidence_id` (`crates/daemon/src/kernel_routes/dispute.rs:55-64`).
- The handler runs `compose_reason` for every operation after
  `resolve_classes` and before the intent, preview, or commit
  (`commit.rs:1139-1153`), and answers `state_only(invalid(reason))`, so
  no transaction starts.
- The module header states the requirement and the refusal names
  (`commit.rs:12-15`, `:46-53`).
- TypeScript: `parseMemoryMarkArgs` prints usage when a dispute cites
  nothing or another event cites something
  (`packages/opencode-plugin/src/shared/memory-mark-command.ts:129-130`).
  The fake kernel mirrors the rule (`disputeEvidenceWellFormed`,
  `packages/opencode-plugin/src/shared/kernel-client-testing/fake-kernel.ts:285-307`,
  applied at `:647-649`).
- No other production writer files the two dispute events: a search of
  `crates/daemon/src`, `crates/kernel/src`, and `packages/*/src` (non-test)
  for `MarkDisputed`, `EventKind::Contradict`, `mark_disputed`, and
  `"contradict"` finds only this route, its client type, its wire list,
  and the command.

## Failure scenario

A client sends `{"op":"disposition","object_id":D,"event":"mark_disputed"}`.
If `compose_reason` were skipped on the preview branch, or `validated`
accepted `(None, None)`, the Kernel would record a dispute with no
evidence and the constant reason, and the subject's effective maturity
would fall to Candidate on assertion alone.

## Timing windows and dependencies

None in time. The check depends on the handler loop running before
`if parsed.preview` (`commit.rs:1181`); it does, so preview and commit
share it.

## What a test must construct

- A dispute with `{}` evidence and with `evidence_id: ""`: `invalid_input`
  from preview and commit, four row counts unchanged.
- `mark_stale` with an `evidence_ref`: `invalid_input`, nothing written.
- `mark_stale`, `explicit_reject`, `quarantine` without evidence: land with
  `reason = "kernel.commit"` and `evidence_id IS NULL`.
- The same shapes through the command parser: usage text.

All four are constructed: `a_dispute_needs_evidence_and_other_events_take_none`
(`crates/daemon/tests/kernel_routes.rs:5971`), the unit test of the same
name (`dispute.rs:393`), and the parser test at
`memory-mark-command.test.ts:126`.

The check is a per-request route oracle (portfolio refinement R4): it
reads each disposition operation's `event`, `evidence_id`, and
`evidence_ref` and the answer. Over stored rows only the dispute clause
has a screen, through the `v=1` reason; for the other three events the
constant reason `kernel.commit` is the only marker of a route-written
row, so a store scan on it is circular.

## Investigation log

### Q: Does the check cover the model tool arm?

- Sources examined: `packages/*/src` for `"disposition"` operations.
- Findings: only the shared command runner sends a disposition at this
  revision; no tool sends one.
- Missing evidence: the #1161 tool arm.
- Conclusion: resolved with answer: the record is `partial - command
  arm`; #1161 extends it.

### Q: Should the Kernel enforce the requirement for every writer?

- Sources examined: `commit.rs:55-58`; `record_admission`
  (`crates/kernel/src/admission.rs:998`).
- Findings: the Kernel accepts a dispute without evidence from any
  in-process caller; the route documents itself as the validator for
  plugin-written disputes and marks its rows with the `v=1` prefix.
- Missing evidence: an owner decision.
- Conclusion: needs human input.
