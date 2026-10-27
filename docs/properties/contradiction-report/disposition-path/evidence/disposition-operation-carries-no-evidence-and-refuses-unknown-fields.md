# disposition-operation-carries-no-evidence-and-refuses-unknown-fields

Catalog record: [catalog.md](../catalog.md#disposition-operation-carries-no-evidence-and-refuses-unknown-fields).
Authored at `3a3b916b7`, 2026-10-25. Status: invalidated by PR A (#1160);
references re-verified at the PR A tip (code commit `6c4d445e3`). The
evidence trail below records the PR A tip; the baseline it replaced is
described in the investigation log.

## Discovery trigger

Plan P4 Problem Frame: "the evidence is dropped at `commit.rs:590-596`".
Lens: protocol contracts and version compatibility. The plan adds
`evidence_id` and `evidence_ref` to the operation in PR A; what a HEAD
daemon does with those keys, and what it writes today, is the baseline
the change is measured against.

## Evidence trail

- `Operation` is `#[serde(tag = "op", rename_all = "snake_case",
  deny_unknown_fields)]` (`crates/daemon/src/kernel_routes/commit.rs:136-137`);
  the `Disposition` variant has `object_id`, `event`,
  `approval_object_id`, and at the PR A tip `evidence_id` and
  `evidence_ref`, plus a `#[serde(skip)]` `reason` (`:155-172`).
- `CommitRequest` is also `deny_unknown_fields` (`:108-109`).
- `parse_request_body` strips the four envelope keys and then runs
  `serde_json::from_value`, mapping any error to
  `invalid_params_error("invalid kernel.commit: ...")`
  (`kernel_routes/mod.rs:463-471`). This runs after
  `kernel_route_scope` binds the project (`mod.rs:452-453`) and before any
  store access, so nothing is written.
- `disposition_request` sets `trigger_object_id: None` (`commit.rs:729`)
  and, at the PR A tip, `evidence_id: evidence_id.map(..)` and the
  composed `reason` (`:731-732`); `admit` sets `None` and the constant
  `"kernel.commit"` for inserts (`:589-594`, constant at `:100`).
- `write_admission` writes those `None`s into `evidence_id` and
  `trigger_object_id` (`admission.rs:1778`, `:1783`).
- Receipt shape: `CommitResult.dispositions` is `#[serde(default,
  skip_serializing_if = "Vec::is_empty")]` (`commit.rs:500-502`), so
  receipts recorded before the field existed still decode; the unit test
  at `:1307` asserts that.
- TypeScript: `DispositionOperation` has `op, object_id, event,
  approval_object_id?`, and at the PR A tip `evidence_id?` and
  `evidence_ref?` (`shared/kernel-client/client.ts:101-111`); the command
  builds the operation with the evidence it parsed
  (`memory-mark-command.ts:209-215`).

## Failure scenario

A PR A client sends `evidence_id` to a HEAD daemon: the daemon answers an
`invalid_params` transport error with no write. If the TypeScript client
maps that to a state the user reads as "refused by the kernel", the user
cannot tell version skew from a kernel refusal. If a HEAD row ever carried
a non-null `evidence_id` for a route disposition, the P3 sweep rule ("keeps
rows a live admission event's evidence cites") would be reading data no
writer at HEAD produces.

## Timing windows and dependencies

None in time. Version skew between the TypeScript bundle and the daemon
binary is the enabling condition.

## What a test must construct

- A raw `kernel.commit` body (not through `KernelClient`) whose
  disposition operation carries `evidence_id`, `evidence_ref`, and an
  arbitrary unknown key; assert the `invalid_params` error and unchanged
  `commit_log`, `admission_decisions`, `operation_receipts` counts.
- After the five-event route test, `SELECT evidence_id, trigger_object_id
  FROM admission_decisions WHERE reason = 'kernel.commit' AND event_kind IN
  (...)`: all null.
- Coverage marker `disposition_body_with_unknown_key_received`.

## Investigation log

### Q: How does the TypeScript client surface a `kernel.commit` `invalid_params` transport error to the `/eidnara-memory-mark` user?

- Sources examined: `memory-mark-command.ts:222-224`, `:238-239`
  (`refused` with the returned state); `client.ts:296-310` (`Invoked`,
  `InvokeOptions`); `kernel-client/state.ts` was not read.
- Findings: the command reports `refused` with whatever `state` the
  client returns; the mapping from a host-call `invalid_params` error to a
  `MemoryState` lives in `client.ts:410-470` and `state.ts`, not read in
  this pass.
- Missing evidence: the exact state key for a transport `invalid_params`
  on a mutating call.
- Conclusion: unresolved, needs a read of `kernel-client/client.ts:410-
  470` and `kernel-client/state.ts`.

### Q: Does the guarantee hold at the PR A tip?

- Sources examined: `commit.rs:155-172`, `:206-234`, `:722-734`; the
  dispute-evidence route tests (`crates/daemon/tests/kernel_routes.rs:5723`,
  `:5971`).
- Findings: no. A dispute row now carries the cited `evidence_id`, and
  the two new keys parse. The other events still write `evidence_id IS
  NULL` with `reason = "kernel.commit"` (asserted at `kernel_routes.rs:5971`),
  `trigger_object_id` stays `None` for every disposition, and any key
  outside the six named fields is still refused by `deny_unknown_fields`.
- Missing evidence: none.
- Conclusion: resolved with answer: `Status: invalidated`; replaced by
  `dispute-requires-evidence-from-plugin` and
  `evidence-ref-well-formed-and-redaction-stable` in
  [`../../dispute-evidence/catalog.md`](../../dispute-evidence/catalog.md).
