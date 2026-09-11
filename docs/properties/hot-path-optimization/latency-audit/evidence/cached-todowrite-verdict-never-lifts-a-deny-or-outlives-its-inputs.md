# cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

All sections from "Discovery trigger" through "Investigation log" describe
pre-change behavior. Their source, test, and check-inventory links pin commit
`ab2ef415`; the scope and provenance link remains live. These sections are not
current implementation claims. The single-flight investigation's source and
test links pin reviewed commit `ff9679ce`, independent of working-tree line
changes.

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
- The hung-read test at [rust-mode-transform.test.ts:466][t466] uses a
  time-based session id and an empty cache. The transform's `?? false` seed
  yields `todo_tool_present: true` after the timeout.
- The capture test at [hook-handlers.test.ts:119][t119] uses the fixed,
  isolated session id `ses-permission-hung`. The failed read finds no cached
  deny, so the capture hook's `cachedToolPermissionDenied` truth check permits
  forwarding. The test asserts one forwarded snapshot, not a
  `todo_tool_present` value.
- The agent-deny test at [:440][t440] and the evaluator test at
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
checks](https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/docs/properties/hot-path-optimization/latency-audit/existing-checks.md#plugin-pre-send)
at `ab2ef415` pinned fail-open on an empty cache; none constructed a
cached-deny-then-failure sequence.

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

[combined]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L77-L107
[combinedseed]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L85-L96
[combinedcatch]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L97-L104
[activeagent]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L68-L75
[failclosed]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1056-L1057
[todowrap]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L310-L316
[permdenied]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L277-L308
[permdisabled]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L203-L213
[permkey]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L71-L73
[permmap]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L57-L59
[clearperm]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L326-L334
[doc17]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L15-L17
[doc57]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L57-L58
[capture]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L270-L292
[agentset]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L133-L135
[hookclient]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/hook.ts#L44-L45
[hookspread]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/hook.ts#L318
[hookclear]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/hook.ts#L375
[evhandler]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/event-handler.ts#L92-L358
[timeout]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/shared/with-timeout.ts#L2
[injection]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/crates/daemon/src/injection.rs#L195-L230
[t466]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L466
[t440]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L440
[t119]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/hook-handlers.test.ts#L119
[t318]: https://github.com/ahrav/eidnara/blob/ab2ef4156b69454b407bd9682d5617ad13c8372f/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L318

## Historical first implementation investigation

This section preserves notes and reported results from the first local
implementation. No separate source snapshot is recorded, so its obsolete line
links are omitted. These notes are unverified history, not evidence of the
reviewed implementation. The single-flight design and retention ledger follow.

### Q: What default and freshness contract does the implementation use?

- Investigation: 2026-09-10, working tree on
  `ab2ef4156b69454b407bd9682d5617ad13c8372f`, branch
  `perf/todowrite-permission-cache`. The discovery narrative and its line
  locators above describe the stated `9132344` baseline, not this working tree.
- User approval provenance: The implementation request explicitly resolves
  the plugin maintainer's open decision: rejection, timeout, missing client,
  or unavailable APIs must report todowrite absent, including empty-cache and
  expired-allow cases. Successful verdicts may be reused for at most 30 seconds
  without a live read. The user accepts silent permission edits remaining
  unobserved within that window because there is no permission-change
  subscription. Freshness invalidates on `session.updated`, native
  `session.compacted`, and `/ctx-flush`; deletion clears entries. Invalidation
  must retain the last deny to preserve the P5 witness.
- Findings: The resolver owns the timeout and fail-closed outcome for both
  transform and capture. The frozen tools-map cache and `ctx_reduce` paths
  remain separate. The `todo_tool_present` wire name and all schemas remain
  unchanged.
- Findings: Keys encode session separately from the JSON tuple
  `(toolName, activeAgent ?? null)` using two SHA-256 hex digests. This preserves
  tuple boundaries, including NULs and lone surrogates, without retaining raw
  identifiers. Unknown agent's JSON null is distinct from every string,
  including the empty string. Identity relies on SHA-256 collision resistance.
- Findings: Freshness uses `performance.now()` at read start,
  before either SDK request. Hits do not extend expiry. The 2,000 ms timeout
  wraps the live evaluator; only the successful bounded await may publish.
  Every new read replaces its entry object. Invalidation sets a negative
  expiry; deletion and eviction remove the entry. Publication checks both
  identity and expiry. A superseded completion returns deny. The underlying
  SDK promises can finish after timeout but cannot write cache state.
- Findings: Session updates, compaction and deletion, and flush invalidate
  only the affected session's permission entries. The scan is bounded by
  2,000 entries and does not refresh LRU order. Failed reads leave the last
  success stale, not fresh; missing-client/API calls produce no success entry.
- Conclusion: resolved with user-approved default and finite freshness bound.

### Q: What retained-resident state does this cache add or resize?

- Sources examined: Entry and cap, keys, bounded map iteration, and eviction
  case in the 2026-09-10 uncommitted working tree based on `ab2ef415`.
- Findings: The permission cache replaces the prior 2,000 Boolean entries;
  it is not a second cache. The entry cap stays 2,000 across all sessions,
  tools, and agents. Each key has 129 ASCII code units: two 64-character
  digests and one separator. The logical key payload ceiling is therefore
  258,000 code units, or 516,000 bytes if charged at two bytes per code unit.
  Values add exactly 4,000 scalar slots total: one Boolean-or-undefined verdict
  and one numeric expiry per entry. No SDK payload, original identifier,
  cached promise, or per-session generation map is retained by the cache.
- Declared retained-resident total adjustment:
  `R_after = R_before - R_old_permission_cache + R_permission_cache`, where
  `R_permission_cache` accounts for at most 2,000 map nodes, 2,000 value
  objects, the bounded key strings above, 4,000 scalar slots, and one map.
  This is a structural retention budget, not an exact VM heap-byte claim.
  String representation, map capacity, object headers, allocator overhead,
  and transient hashing/SDK allocations are unmeasured. No process RSS or
  numeric aggregate heap total is claimed.
- Retention: Stale successes and failed-read placeholders remain until LRU
  eviction or deletion. They never qualify as fresh successes. In-flight SDK
  work is not cached or coalesced and keeps its existing timeout limitation:
  timeout bounds the hook wait, not the lifetime of an uncooperative SDK call.
- Conclusion: entry count, identifier payload, scalar count, and retention
  policy are bounded; runtime heap overhead remains unmeasured.

### Q: What did local verification establish?

- Fresh proof-first hook command:
  `bun run --cwd packages/opencode-plugin test src/hooks/context/hook.test.ts --test-name-pattern 'cached deny and live|shares fresh permission|permission client is unavailable'`.
  Result before production edits: 2 passed, 2 failed. A shared-hit test saw
  three SDK reads instead of one; missing-client capture forwarded a snapshot.
- Focused command:
  `bun run --cwd packages/opencode-plugin test src/hooks/context/ctx-reduce-availability.test.ts src/hooks/context/hook.test.ts src/hooks/context/hook-handlers.test.ts src/hooks/context/rust-mode-transform.test.ts src/hooks/context/event-handler.test.ts src/shared/bounded-session-map.test.ts`.
  First integration run: 157 passed, 4 failed. Two fixtures still assumed the
  old fail-open default; two assertions counted detached capture sends before
  their promise continuations drained. Explicit successful SDK fixtures and
  the existing next-tick drain pattern fix those setup errors.
- Final focused run: 162 passed, 0 failed, 635 assertions on Bun 1.3.14.
  Hook cases cover both consumers and the constant P5 marker; resolver cases
  cover identities, TTL, failure states, late completions, and LRU eviction.
- `bun run --cwd packages/opencode-plugin typecheck` passes.
  `bun run --cwd packages/opencode-plugin lint --diagnostic-level=error`
  passes. An earlier lint run found five formatting errors, corrected by
  scoped patches. The unfiltered package lint also passes and reports 49
  existing unrelated warnings.
  `scripts/forbid-comment-markers.sh` passes.
- Documentation checks: All 29 added file/line links resolve to the inspected
  working-tree ranges. P1 and P5 retain `Status: active` and now record
  `Exercised: yes`. Historical evidence remains intact; the evidence files
  exceed the method's length target to preserve it.
- Missing evidence: The outer orchestrator owns full `bun run check:repo`,
  plugin smoke, independent reviews, and shipping. This campaign is not a
  production-shape latency comparison or a merge verdict.
- Conclusion: the constructed P1 cases pass under the approved contract.

## Single-flight investigation

### Q: Can overlapping successful reads invent a deny?

- Sources examined: [resolver overlap][singleflight-overlap],
  [transform/capture overlap][singleflight-hook], and
  [shared resolver][singleflight-resolver].
- Proof-first result: The command
  `bun run --cwd packages/opencode-plugin test src/hooks/context/ctx-reduce-availability.test.ts src/hooks/context/hook.test.ts --test-name-pattern 'overlapping same-key|overlapping transform and capture'`
  runs three tests before the single-flight fix: 0 passed, 3 failed, 7
  assertions. The resolver returns `[true, false]` rather than `[false, false]`.
  Both hook variants send `todo_tool_present: false` instead of true.
- Findings: Each invalidation-free key shares one pending promise. Followers
  join its existing 2,000 ms timeout. Invalidation sets an explicit flag and
  the next read replaces the entry, retaining its last successful verdict.
  Deletion and global LRU eviction remove the entry. Publication checks
  entry identity, invalidation, and read-start TTL; each waiter checks again
  before returning. No fresh-state enum or separate generation map is needed.
- Findings: A clock-only settlement test advances `performance.now()` by
  30,000 ms without running timers. The successful SDK response cannot publish
  or return allow even when the timeout callback has not run. Pending-entry
  eviction denies both waiters and cannot repopulate the key. An invalidated
  fill does not block a fresh fill for that key.
- Conclusion: resolved. Successful same-key overlap shares the live answer;
  revoked or expired work cannot publish or return allow.

### Q: What permission evidence and observation APIs apply?

- Findings: The [typed SDK reader][singleflight-reader] rejects missing named
  agents, malformed response shapes, and non-null SDK errors. Undefined agent
  evaluates session rules alone. The core key distinguishes undefined from all
  strings, but host hooks normalize empty agent strings to undefined. Distinct
  real agent inputs remain distinct keys. The missing-client/API gate logs and
  denies even when the cache contains a fresh allow.
- Findings: The private reader takes the nonoptional typed client and has no
  duplicate API availability guard. The shared resolver owns fallback and
  logging. `peekToolPermissionDeniedForTest` requires the explicit agent input;
  only tests call it. It observes last successful state without claiming
  freshness or touching LRU order.
- P5 scope: The marker proves cached-deny-plus-live-failure reachability.
  It does not prove that cached deny causes a different fallback result:
  fail-closed empty-cache and expired-allow cases also deny.
- Conclusion: resolved under the user's explicit policy. Every
  `session.updated` still invalidates freshness; the policy is not narrowed
  to permission-only changes.

### Q: What is the current retained-state ledger?

- Sources examined: [entry fields and cap][singleflight-cap],
  [key encoding][singleflight-keys], and [lifetime tests][singleflight-lifetime].
- Settled cache: At most 2,000 entries globally, each with a 129-code-unit
  digest key, three scalar fields (`denied`, `expiresAt`, `invalidated`), and
  one optional pending-promise reference. Logical key payload remains at most
  258,000 code units, charged as 516,000 bytes at two bytes per code unit.
  This means 6,000 scalar fields and 2,000 reference slots, plus map nodes,
  value objects, and string/container overhead. Settled entries do not retain
  SDK payloads or raw identifiers through a pending promise.
- Pending cache: At most one shared fill promise per retained entry, at most
  2,000 such references globally. Fill promises, timeout state, waiter
  continuations, and request/SDK state add dynamic memory. Raw identifiers and
  SDK responses can remain live during a fill. A reference cap is not a byte
  bound on this request-owned data or on the number of callers waiting.
- Declared total adjustment:
  `R_after = R_before - R_old_permission_cache + R_permission_structure + R_pending_fills`.
  `R_permission_structure` includes the fixed payload and field counts above;
  `R_pending_fills` includes the retained promises and their dynamic state.
  Neither VM heap overhead nor total process RSS is measured. This ledger
  does not claim an exact byte total or a physical in-flight memory bound.
- Limits: Invalidation and deletion scan the global cache, at most 2,000
  entries. All sessions and agents compete for that cap; eviction can discard
  another session's fresh verdict or revoke its pending fill. Failed reads
  do not become fresh cache entries, so a later call retries. No backoff or
  failure cache is introduced. Timeout bounds the logical wait, not the
  lifetime of uncooperative SDK work. Invalidated or evicted fills may still
  exist outside the cache; there is no global physical-work admission limit.
- Conclusion: the settled footprint and retained fill-reference count are
  bounded. Dynamic request memory and VM overhead remain unmeasured. No
  latency benefit is claimed for hashing, cache hits, or invalidation scans.

### Q: What verification follows the single-flight fix?

- The same six-file focused command recorded above passes 169 tests, 0
  failures, 675 assertions on Bun 1.3.14. An intermediate run passed 168 tests
  with 668 assertions; lint then reported four format/import-order errors,
  corrected by scoped patches before the final run.
- `bun run --cwd packages/opencode-plugin typecheck`,
  `bun run --cwd packages/opencode-plugin lint --diagnostic-level=error`,
  `scripts/forbid-comment-markers.sh`, and `git diff --check` pass.
- Test types: Package typecheck excludes tests. A supplemental TypeScript API
  program adds the six focused test files to the package configuration and
  compares diagnostics with HEAD source supplied through an in-memory compiler
  host. HEAD and the working tree each report two diagnostics; there are no new
  diagnostic signatures. Both remaining errors are TS2352 and TS2493 at
  `hook.test.ts:1297`, the preexisting `promptMock.mock.calls[0]?.[0]` assertion.
  The tests do not have a clean typecheck result. A transient production
  `boolean | undefined` return diagnostic was fixed before package typecheck.
- Missing evidence: Full repo checks, smoke, independent review, and shipping
  remain owned by the outer orchestrator. Historical execution results above
  are retained rather than rewritten to match this revision.
- Conclusion: all required local runtime and package gates pass; the two
  preexisting test-type diagnostics remain explicit.

[singleflight-overlap]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L368-L391
[singleflight-hook]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/hook.test.ts#L408-L465
[singleflight-resolver]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L295-L351
[singleflight-reader]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L353-L383
[singleflight-cap]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L59-L68
[singleflight-keys]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L80-L91
[singleflight-lifetime]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L347-L604

### Q: How does an empty host agent reach the permission cache?

- The host boundary treats an empty agent as absent. Both the
  [transform extractor][host-agent-transform] and [capture hook][host-agent-capture]
  normalize it to undefined. The core digest scheme and its string-collision
  tests remain unchanged; an empty host value is not a fictitious named agent.
- The [shared-fill fixture][singleflight-hook] uses SDK `agents: []` for the
  empty host case. Session allow and deny rules determine the outcome. An
  undefined capture joins the empty-agent transform's fill, and a subsequent
  empty-agent capture reuses it with no second SDK read.
- Proof-first hook run: 1 passed, 2 failed, 13 assertions. The empty-agent allow
  case returned false; the deny case performed three SDK reads instead of one.
  After normalization, the six-file focused family passes 170 tests, 0
  failures, 680 assertions. The existing database host-agent normalization
  test also passes: 1 test, 2 assertions.
- Package typecheck and lint pass. These package commands exclude test type
  checking; the earlier supplemental diagnostic findings remain historical.
  The outer orchestrator reports that full `check:repo` and smoke passed before
  this boundary correction, including 3,744 plugin tests. Those full gates
  were not rerun locally for this correction.
- Conclusion: empty host agent and undefined use session rules and one shared
  key; nonempty real agent names remain distinct.

[host-agent-transform]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L66-L73
[host-agent-capture]: https://github.com/ahrav/eidnara/blob/ff9679ceb41bbd43b9e169dee210eff7e58c9b26/packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L264-L265
