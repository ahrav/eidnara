# p5dep-memory-tool-actions-are-closed-and-allowlisted

HEAD `3a3b916b7`. Surface S8.

## Discovery trigger

Plan R6/U4: the `eidnara_memory` tool gains a `candidates` action (a read).
P4 #1159 D8 adds `dispute` (a mutation) and OQ4-11 names default allowlist
membership as the rollout lever; N3 records the prompt-injection risk. The
task names the claims: closed action list, read vs mutation classification,
allowlist semantics, context_researcher exclusion. Lens: security boundaries;
C14; D7.

## Evidence trail

- `packages/opencode-plugin/src/tools/eidnara-memory/types.ts:6`
  `EIDNARA_MEMORY_ACTIONS = ["create", "get", "revise", "archive", "merge"] as
  const`; `:8` the action type; `:10` `EIDNARA_MEMORY_READ_ACTIONS = new
  Set(["get"])`; `:12-15` `isEidnaraMemoryMutation(action) =
  !READ_ACTIONS.has(action)`; `:31-37` `EidnaraMemoryToolDeps.allowedActions?:
  EidnaraMemoryAction[]`.
- `tools.ts:45-49`: the args schema's `action` is `enum([...ACTIONS])`,
  optional; `:70` `.passthrough()` on the object schema (unknown keys are
  tolerated at schema level; the action gate is explicit code).
- `tools.ts:72-74` `allowedActions(deps) = deps.allowedActions ??
  EIDNARA_MEMORY_ACTIONS`; doc: omitted admits every action, explicit empty
  admits none.
- `tools.ts:77-101` execute: parse; `unwrapImitatedReducedArgs`; `approve` or
  `enforce` -> "human-host-owned commands" error (`:88-90`); `toolContext.agent
  === CONTEXT_RESEARCHER_AGENT` -> "not available to the context_researcher
  agent" (`:91-93`); `typeof rawAction !== "string" ||
  !ACTIONS.includes(rawAction) || !primaryActions.includes(rawAction)` ->
  "Action '<a>' is not allowed in this context." (`:94-101`). All three
  return before `deps.kernelClient` is called (`:118`).
- `tools.ts:115-117`: `!toolCallId && isEidnaraMemoryMutation(action)` ->
  "mutation requires a stable tool-call identity"; `:128` reads use
  `toolCallId ?? "read"`.
- `packages/opencode-plugin/src/agents/context-researcher.ts:1`
  `CONTEXT_RESEARCHER_AGENT = "context-researcher"`.
- `packages/opencode-plugin/src/plugin/tool-registry.ts:58`: `allowedActions:
  [...EIDNARA_MEMORY_ACTIONS]` (the full list at registration).
- `packages/pi-plugin/src/tools/eidnara-memory.ts:57` `enum:
  EIDNARA_MEMORY_ACTIONS`; `:128-140` approve/enforce and closed-list checks;
  `:152-155` mutation identity check. No `allowedActions` and no agent check.
- `packages/pi-plugin/src/tools/index.ts:30-32` `memoryToolEnabled` doc: "The
  context_researcher needs read-only `eidnara_search`; the main agent keeps
  `eidnara_memory`"; `:69` `if (opts.memoryToolEnabled !== false)` registers
  it; `packages/pi-plugin/src/subagent-entry.ts:63` `memoryToolEnabled:
  false`; `packages/pi-plugin/src/index.ts:475` `memoryToolEnabled: true`.
- Tests: `tools.test.ts:1157-1201` allowlist suite (empty list refuses every
  action with zero transport calls; omitted admits `get`); `:1150-1153`
  `enforce` and `delete` refusals; `packages/pi-plugin/src/tools/index.test.ts:40-70`
  registration matrix (context_researcher case registers `eidnara_search`
  and `todowrite` only).

## Failure scenario

If `candidates` or `dispute` were added to the enum but not to the read set
correctly, a `candidates` call without a tool-call id would be refused as a
mutation (over-strict) or a `dispute` without one would commit under the
`"read"` identity (under-strict, breaking P4's idempotency key). If the pi
plugin shipped `dispute` with no allowlist, the P4 rollout lever would not
exist there (D7). If the opencode agent check were removed, a
context-researcher subagent (which receives untrusted retrieved text) could
file disputes (P4 N3).

## Timing windows and dependencies

None; synchronous guards before any network call.

## What a test must construct

- Each action under empty, partial (`["get"]`), and omitted allowlists with a
  fake transport counting calls.
- `toolContext.agent = "context-researcher"` with a valid action; assert the
  refusal text and zero calls (not present at HEAD).
- Pi: `memoryToolEnabled: false` registration (existing); a disallowed
  action string on the pi tool (closed-list refusal).
- After P4/P5: `isEidnaraMemoryMutation("dispute") === true`,
  `isEidnaraMemoryMutation("candidates") === false`.

## Investigation log

### Q: D7. Should the pi tool gain `allowedActions`?

- Sources examined: `eidnara-memory.ts:128-155`, `index.ts:30-32`.
- Findings: pi gates by registration only; every registered action is
  callable. P4 OQ4-11's lever is opencode-only at HEAD.
- Missing evidence: author intent for parity.
- Conclusion: needs human input.
