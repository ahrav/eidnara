# pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false

## Discovery trigger

Lens: architecture (S5) and claimed guarantees (P4). The brief named the
pass-input builders and the shared response type. KTD8 states "Pi keeps
`is_subagent: false` because the extension API exposes `parentSession` only
at `newSession`, not per context event; the asymmetry is documented." R13
parity is "for the same `is_subagent` value", so the builders are where the
two harnesses' requests agree or differ.

## Evidence trail

- Shared builder: `packages/opencode-plugin/src/hooks/context/transform-pass-fields.ts:1-4`
  ("Both harness adapters build their requests' fields here"); `:55-100`
  `transformPassFields` returns the fixed key set `render_config`,
  `system_prompt_hash`, `upgrade_state`, `is_subagent`, `protected_tags`,
  optional `usage`, `prev_response_cache_usage`, `geometry`, `mid_turn`,
  `prev_response_completed_at_ms`, `request_observed_at_ms`,
  `channel2_nudge_state`, `emergency_recovery_armed`,
  `emergency_recovery_no_head_escape`, `model_key`, `provider_id`,
  `tool_present`, optional `todo_tool_present`, the prompt-surface fields,
  `effective_execute_threshold`, the auto-search trio,
  `history_budget_tokens`, `clear_reasoning_age`, the terse pair,
  `cache_ttl`.
- Coercion: `:66` `is_subagent: args.passInputs.is_subagent === true`.
- Pi: `packages/pi-plugin/src/transform/pi-pass-inputs.ts:71-128`
  `piPassFields`; `:110` `is_subagent: false`; `:111` `tool_present:
  args.reduceRegistered` (passed as `true` at `index.ts:594`).
- OpenCode: `packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts:458`
  `const isSubagent = deps.isSubagentSession(sessionId)`; `:507`
  `is_subagent: isSubagent`; `:502-503` disables terse compression for
  subagents client-side as well.
- Wire: `docs/host-wire-protocol.md:966` `is_subagent` optional, default
  `false`.
- Daemon consumers: `transform.rs:4216-4222`, `:11723-11733`, `:3627`,
  `:3830`, `:5190`.
- Shared client: `transform-session-client.ts:464-468` `responseValue`
  returns `Record<string, unknown>`; no TypeScript type names
  `project_memory` (grep over `packages/` finds it only in the e2e test).
- Tests: `hook.test.ts:1790-1803` asserts `is_subagent: true` for an
  OpenCode restored child; `pi-pass-inputs.test.ts:39-95` asserts usage and
  completion fields only.
- Pi children: `index.ts:332-336` (spawned child, env guard) and `:338-348`
  (in-process child, latch) skip the full factory; the parent's one
  `context` handler is the only Pi sender.

## Failure scenario

Parity (AE8) compares Pi and OpenCode "with the same `cwd` and window". A
Pi in-process child session and an OpenCode child session for the same
project receive different rule sets: Pi's child is a non-subagent request
and gets m0; OpenCode's child is a subagent request and gets none on the
fold path. A future Pi change inferring the flag per event would change
which sessions receive rules without any wire change.

## Timing windows and dependencies

None; the builders are synchronous. Depends on `isSubagentSession` being
correct on the OpenCode side (owned by that adapter's tests).

## What a test must construct

1. `piPassFields(args)` for a top-level session and for a session id
   created by an in-process child; assert `is_subagent === false` in both.
2. `transformPassFields({ passInputs: { is_subagent: "true" }, ... })`;
   assert `is_subagent === false` (strict coercion).
3. Build both harnesses' outputs from the same `passInputs` shape and
   assert equal key sets, with the known differing values (`is_subagent`,
   `tool_present`) listed explicitly.
4. Coverage: `pi_pass_fields_built_for_child_session` fires in case 1.

## Investigation log

### Q: Does the parent Pi process's `context` handler observe pi-subagents in-process child sessions?

- Sources examined: `index.ts:338-348`, `:566-609`; `subagent-entry.ts`
  header; `index-in-process-latch.test.ts:43-100`.
- Findings: the child's factory call is skipped by the latch, so no second
  handler exists; whether Pi dispatches the parent extension's `context`
  handler for a child session is a property of the Pi host, not visible in
  this repository.
- Missing evidence: Pi host dispatch semantics or an e2e with an in-process
  child capturing its provider request.
- Conclusion: partial: skip of the child factory is confirmed; handler scope
  per session is unresolved.
