# disposition-operation-carries-no-evidence-and-refuses-unknown-fields

Catalog record: [catalog.md](../catalog.md#disposition-operation-carries-no-evidence-and-refuses-unknown-fields).
HEAD `3a3b916b7`, 2026-10-25.

## Discovery trigger

Plan P4 Problem Frame: "the evidence is dropped at `commit.rs:590-596`".
Lens: protocol contracts and version compatibility. The plan adds
`evidence_id` and `evidence_ref` to the operation in PR A; what a HEAD
daemon does with those keys, and what it writes today, is the baseline
the change is measured against.

## Evidence trail

- `Operation` is `#[serde(tag = "op", rename_all = "snake_case",
  deny_unknown_fields)]` (`crates/daemon/src/kernel_routes/commit.rs:82-83`);
  the `Disposition` variant has three fields (`:101-105`).
- `CommitRequest` is also `deny_unknown_fields` (`:54-55`).
- `parse_request_body` strips the four envelope keys and then runs
  `serde_json::from_value`, mapping any error to
  `invalid_params_error("invalid kernel.commit: ...")`
  (`kernel_routes/mod.rs:462-470`). This runs after
  `kernel_route_scope` binds the project (`mod.rs:451-452`) and before any
  store access, so nothing is written.
- `disposition_request` sets `trigger_object_id: None` and `evidence_id:
  None` (`commit.rs:593-595`); `admit` does the same for inserts
  (`:461-466`). The reason is the constant `"kernel.commit"` (`:46`).
- `write_admission` writes those `None`s into `evidence_id` and
  `trigger_object_id` (`admission.rs:1773`, `:1778`).
- Receipt shape: `CommitResult.dispositions` is `#[serde(default,
  skip_serializing_if = "Vec::is_empty")]` (`commit.rs:372-374`), so
  receipts recorded before the field existed still decode; the unit test
  at `:1171` asserts that.
- TypeScript: `DispositionOperation` has `op, object_id, event,
  approval_object_id?` (`shared/kernel-client/client.ts:95-101`); the
  command builds `{op, object_id, event}` (`memory-mark-command.ts:115-
  119`).

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

- Sources examined: `memory-mark-command.ts:126-128`, `:142-143`
  (`refused` with the returned state); `client.ts:286-300` (`Invoked`,
  `InvokeOptions`); `kernel-client/state.ts` was not read.
- Findings: the command reports `refused` with whatever `state` the
  client returns; the mapping from a host-call `invalid_params` error to a
  `MemoryState` lives in `client.ts:400-460` and `state.ts`, not read in
  this pass.
- Missing evidence: the exact state key for a transport `invalid_params`
  on a mutating call.
- Conclusion: unresolved, needs a read of `kernel-client/client.ts:400-
  460` and `kernel-client/state.ts`.
