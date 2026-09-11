# cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit counts two SDK calls per pass for the `todowrite` permission read
and proposes serving a cached verdict. The read already has a cache, but the
cache is a fallback, not a source, and its key omits one of the live call's
inputs. Any design that promotes the cache to a source inherits both facts.

## Evidence trail

- [`resolveCombinedTodowriteVerdict`][combined] returns early when the frozen
  map verdict is not callable or `compactionOff` is set ([`:83`][combined]),
  then seeds `permissionDenied` from
  [`cachedToolPermissionDenied(sessionId, "todowrite") ?? false`][combinedseed]
  and, when `deps.client` is set, awaits [`todowritePermissionDenied`][todowrap]
  under [`withTimeout(..., HOST_SDK_READ_TIMEOUT_MS)`][combinedseed].
- [`todowritePermissionDenied`][todowrap] forwards to
  [`resolveToolPermissionDenied`][permdenied], which runs `app.agents()` and
  `session.get()` in one `Promise.all`, looks the active agent up by name,
  evaluates [`permissionDisabled`][permdisabled] over agent rules then session
  rules, and writes the result under
  [`permissionCacheKey(tool, session)`][permkey] into a 2000-entry
  [`BoundedSessionMap`][permmap].
- The catch at [`:97-104`][combinedcatch] logs and leaves the seeded value in
  place; [`HOST_SDK_READ_TIMEOUT_MS`][timeout] is 2000 ms.
- [`activeAgentFromMessages`][activeagent] reads `info.agent` from the last
  user message. It is an argument to the live call and absent from the key.
- The only invalidation is [`clearToolPermissionDenied`][clearperm], called
  from the `session.deleted` handler at [hook.ts:375][hookclear]. The event
  handler dispatches `session.created`, `session.error`, `message.updated`,
  `message.removed`, `session.compacted`, and `session.deleted`
  ([event-handler.ts][evhandler]); no `permission.*` type is matched.
- The [capture hook][capture] shares the cache: it reads the live verdict
  and, on a failed read, returns early only when a cached deny exists.
- `deps.client` is `deps.client` of the hook's [`EidnaraDeps`][hookclient],
  spread into the transform deps at [hook.ts:318][hookspread], so the live
  read runs on every production pass.
- The host consumes `todo_tool_present` in
  [`capture_todo_state_on_bust`][injection] and
  `advance_injection_from_meta`; `Some(false)` suppresses capture and treats a
  frozen pair as empty. The verdict changes prompt bytes, not authorization.
- The hung-read tests at [rust-mode-transform.test.ts:466][t466] and
  [hook-handlers.test.ts:119][t119] use fresh session ids, so the cache is
  empty and `?? false` yields `todo_tool_present: true`. The agent-deny test at
  [:440][t440] and the evaluator test at
  [ctx-reduce-availability.test.ts:318][t318] show the agent input changes the
  answer.

## Failure scenario

A pass stores `true` for `(todowrite, S)`. A later pass's SDK read rejects or
hangs past 2000 ms. A rewrite that stores a default on miss, returns early on
a hit, or reads the cache after the catch sends `todo_tool_present: true`, and
the host injects a synthetic pair the harness cannot execute. Separately, a
cache promoted to a source serves agent A's deny for a pass whose last user
message names agent B, or serves a verdict computed before a session
permission edit, with no event to invalidate it.

## Timing windows and dependencies

The fallback clause is evaluated only when the read fails, which needs a slow
or failing OpenCode server. The staleness clause needs two passes on one
session with different `info.agent` values ([`agentBySession`][agentset] is
set per `chat.message`) or a permission edit between passes. The 2000-entry
LRU can evict an entry between passes, which turns a cached deny into a miss.

## What a test must construct

An SDK fake that answers `deny` once, then rejects or hangs; a frozen callable
map verdict; then assert `todo_tool_present: false` on the failed pass. For
staleness, two passes whose last user messages carry agents with opposite
`todowrite` rules, comparing the served verdict with `permissionDisabled` over
the fake's current rules. The [plugin
checks](../existing-checks.md#plugin-pre-send) pin fail-open on an empty cache
only; none constructs a cached deny.

## Investigation log

### Q: Is fail-open on an empty cache and a failed read the intended default?

- Sources examined: [`?? false`][combinedseed]; the doc comments at
  [ctx-reduce-availability.ts:15-17][doc17] and [:57-58][doc57]; the comment
  at [rust-mode-transform.ts:1056-1057][failclosed]; [t466] and [t119].
- Findings: The code defaults to "not denied". The comment at 1057 says
  synthesis fails closed when evidence is missing; the availability doc says
  the live read runs only at cache-busting boundaries and defer passes reuse
  the cache, while [`:85-96`][combinedseed] reads live on every pass. Both
  tests pin the fail-open outcome under names that say "cached verdict".
- Missing evidence: A written decision on the default.
- Conclusion: needs human input.

### Q: What staleness is acceptable, and which events must invalidate?

- Sources examined: [`permissionCacheKey`][permkey], [`clearperm`][clearperm],
  the event dispatch in [event-handler.ts][evhandler].
- Findings: Only `session.deleted` and LRU eviction remove entries. The key
  has no agent component.
- Missing evidence: A staleness budget in passes or time.
- Conclusion: needs human input.

### Q: Does the SDK emit a permission-change event?

- Sources examined: The plugin's event handler and `hook.ts`.
- Findings: No `permission.*` string appears in the plugin source.
- Missing evidence: The SDK event list for the pinned OpenCode version, which
  is not in this repository.
- Conclusion: unresolved, needs the SDK event list.

[combined]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L77-L107
[combinedseed]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L85-L96
[combinedcatch]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L97-L104
[activeagent]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L68-L75
[failclosed]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1056-L1057
[todowrap]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L310-L316
[permdenied]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L277-L308
[permdisabled]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L203-L213
[permkey]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L71-L73
[permmap]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L57-L59
[clearperm]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L326-L334
[doc17]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L15-L17
[doc57]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L57-L58
[capture]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L270-L292
[agentset]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L133-L135
[hookclient]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.ts#L138-L139
[hookspread]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.ts#L318
[hookclear]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.ts#L375
[evhandler]: ../../../../../packages/opencode-plugin/src/hooks/context/event-handler.ts#L92-L358
[timeout]: ../../../../../packages/opencode-plugin/src/shared/with-timeout.ts#L2
[injection]: ../../../../../crates/daemon/src/injection.rs#L195-L230
[t466]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L466
[t440]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L440
[t119]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.test.ts#L119
[t318]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L318
