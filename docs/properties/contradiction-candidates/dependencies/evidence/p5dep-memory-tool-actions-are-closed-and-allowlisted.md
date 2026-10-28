# p5dep-memory-tool-actions-are-closed-and-allowlisted

Authored at `3a3b916b7`; re-verified at the #1164 branch tip. Surface S8.

## Discovery trigger

Plan R6/U4: the `eidnara_memory` tool gains a `candidates` action (a read).
P4 #1159 D8 adds `dispute` (a mutation) and OQ4-11 names default allowlist
membership as the rollout lever; N3 records the prompt-injection risk. The
task names the claims: closed action list, read vs mutation classification,
allowlist semantics, context_researcher exclusion. Lens: security boundaries;
C14; D7.

## Evidence trail

References re-verified at the #1168 branch tip (base `cda655423`); #1168
added the `candidates` read. #1161 added the `dispute` action, `EIDNARA_MEMORY_DEFAULT_ACTIONS`, per-allowlist
schemas, and the pi `allowedActions` since `3a3b916b7`.

- `packages/opencode-plugin/src/tools/eidnara-memory/types.ts:6-14`
  `EIDNARA_MEMORY_ACTIONS = ["create", "get", "revise", "archive", "merge",
  "dispute", "candidates"] as const`; `:16` the action type; `:19-20`
  `EIDNARA_MEMORY_DEFAULT_ACTIONS` filters out `dispute`; `:23`
  `EIDNARA_MEMORY_DISPUTE_EVENTS`; `:25-28` `EIDNARA_MEMORY_READ_ACTIONS = new
  Set(["get", "candidates"])`; `:31-33` `isEidnaraMemoryMutation(action) =
  !READ_ACTIONS.has(action)`; `EidnaraMemoryToolDeps` with
  `allowedActions?: EidnaraMemoryAction[]` (`:60-66`).
- `tools.ts:48-58` `actionField`: the `action` enum lists exactly the admitted
  actions; `:82-100` the dispute-only fields and `:103-116` the
  `candidates` paging fields; `:118-120` the parse schema with
  `.passthrough()` (unknown keys are tolerated at schema level; the action
  gate is explicit code); `:131-138` the exposed args include the paging
  and dispute fields only when the allowlist names `candidates` or
  `dispute`.
- `tools.ts:122-125` `allowedActions(deps) = deps.allowedActions ??
  EIDNARA_MEMORY_DEFAULT_ACTIONS`; doc: omitted admits the default actions,
  which exclude `dispute`; explicit empty admits none.
- `tools.ts:139-190` execute: parse; `unwrapImitatedReducedArgs`; `approve` or
  `enforce` -> "human-host-owned commands" error (`:145-147`);
  `toolContext.agent === CONTEXT_RESEARCHER_AGENT` -> "not available to the
  context_researcher agent" (`:148-150`); `typeof rawAction !== "string" ||
  !ACTIONS.includes(rawAction) || !primaryActions.includes(rawAction)` ->
  "Action '<a>' is not allowed in this context." (`:151-157`). All three
  return before `deps.kernelClient` is called (`:175`).
- `tools.ts:172-174`: `!toolCallId && isEidnaraMemoryMutation(action)` ->
  "mutation requires a stable tool-call identity"; `:185` reads use
  `toolCallId ?? "read"`.
- `packages/opencode-plugin/src/agents/context-researcher.ts:1`
  `CONTEXT_RESEARCHER_AGENT = "context-researcher"`.
- `packages/opencode-plugin/src/plugin/tool-registry.ts:58`: `allowedActions:
  [...EIDNARA_MEMORY_DEFAULT_ACTIONS]` at registration, so `dispute` is off by
  default.
- `packages/pi-plugin/src/tools/eidnara-memory.ts:114-126` `paramsSchema`: the
  `action` enum lists the admitted actions and the dispute fields appear only
  when `dispute` is admitted; `:146-152` `allowedActions` defaults to
  `EIDNARA_MEMORY_DEFAULT_ACTIONS`; `:174-186` approve/enforce and allowlist
  checks; `:198-202` mutation identity check. No agent check: pi excludes the
  context_researcher by registration.
- `packages/pi-plugin/src/tools/index.ts:30-32` `memoryToolEnabled` doc: "The
  context_researcher needs read-only `eidnara_search`; the main agent keeps
  `eidnara_memory`"; `:69-75` `if (opts.memoryToolEnabled !== false)`
  registers it with no `allowedActions`, so the default applies;
  `packages/pi-plugin/src/subagent-entry.ts:63` `memoryToolEnabled: false`;
  `packages/pi-plugin/src/index.ts:475` `memoryToolEnabled: true`.
- Tests (opencode, `tools.test.ts`): `:1149-1160` approve, enforce, and
  `delete` refusals; `:1162-1234` allowlist suite (`:1185` empty list refuses
  every action with zero transport calls; `:1197` omitted list refuses
  `dispute`; `:1213` omitted list admits `get`; `:1223` partial list);
  `:1338-1354` `dispute` without a tool-call id, from the context-researcher
  agent, and under a `["get"]` allowlist, all with zero transport calls, and
  `EIDNARA_MEMORY_DEFAULT_ACTIONS` excludes `dispute`.
- Tests (pi): `packages/pi-plugin/src/tools/eidnara-memory.test.ts:303-326`
  `list`, `approve`, `enforce`, and a malformed create refused with zero
  transport calls; `:422-431` the default schema omits `dispute` and the
  default allowlist refuses it; `packages/pi-plugin/src/tools/index.test.ts:40-70`
  registration matrix (the context_researcher case registers
  `eidnara_search` and `todowrite` only).

## Failure scenario

If `candidates` were added to the enum but not to the read set, or
`dispute` were added to the read set, a `candidates` call without a
tool-call id would be refused as a mutation (over-strict) or a `dispute` without one would commit under the
`"read"` identity (under-strict, breaking P4's idempotency key). If either
plugin's default allowlist admitted `dispute`, the P4 rollout lever would be
spent at install (D7). If the opencode agent check were removed, a
context-researcher subagent (which receives untrusted retrieved text) could
file disputes (P4 N3).

## Timing windows and dependencies

None; synchronous guards before any network call.

## What a test must construct

- Each action under empty, partial (`["get"]`), and omitted allowlists with a
  fake transport counting calls.
- `toolContext.agent = "context-researcher"` with a valid action; assert the
  refusal text and zero calls (present for `dispute`, `tools.test.ts:1341`).
- Pi: `memoryToolEnabled: false` registration and a disallowed action string
  (both present).
- `get` without a tool-call id runs under the `"read"` identity (not found).
- After P5 U4: `isEidnaraMemoryMutation("candidates") === false`.

## Investigation log

### Q: D7. Should the pi tool gain `allowedActions`?

- Sources examined: `eidnara-memory.ts:143-189`, `index.ts:30-32`, `:69-75`,
  `types.ts:18-20`.
- Findings: at `3a3b916b7` pi gated by registration only. #1161 added
  `allowedActions` to the pi tool with the opencode default
  (`EIDNARA_MEMORY_DEFAULT_ACTIONS`, which excludes `dispute`), so the P4
  rollout lever exists in both plugins.
- Missing evidence: none for D7. At the #1164 tip the default list's
  `candidates` membership was undecided; #1168 places `candidates` in
  `EIDNARA_MEMORY_DEFAULT_ACTIONS`, so `dispute` is the one action an
  omitted allowlist withholds (`types.ts:18-20`).
- Conclusion: resolved with answer: both plugins carry the allowlist, and
  `candidates` is in the default list of both.
