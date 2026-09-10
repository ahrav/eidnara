# todowrite-deny-then-read-failure-is-exercised

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

Historical reference warning: The discovery trail and initial investigation
below use pre-implementation line numbers. Those relative source/test links
are stale against the working tree; inspect
`ab2ef4156b69454b407bd9682d5617ad13c8372f` for the pre-change code. The final
single-flight investigation has current links and supersedes the causal
fallback rationale, not the historical execution results.

## Discovery trigger

P1's first clause holds only on passes where the live read fails and a cached
deny exists. Both existing hung-read tests start with an empty cache, so the
branch that preserves a deny has never run with a deny to preserve. A green
suite proves nothing about it. This record asks a campaign to witness the two
preconditions, not the outcome.

## Evidence trail

- [`resolveCombinedTodowriteVerdict`][combined] seeds from
  [`cachedToolPermissionDenied`][combinedseed] before the read; the catch at
  [`:97-104`][combinedcatch] leaves the seed in place.
- A successful read writes the verdict at
  [`resolveToolPermissionDenied:306`][permstore] under
  [`permissionCacheKey(tool, session)`][permkey]; a stored `true` is the deny
  the fallback must keep.
- The read is a `Promise.all` over `app.agents()` and `session.get()`
  ([`:291-294`][permdenied]) raced against
  [`HOST_SDK_READ_TIMEOUT_MS = 2000`][timeout]; either rejection or the timer
  enters the catch.
- The function returns before the read when `availability.frozen` or
  `availability.callable` is false or `compactionOff` is true
  ([`:83`][combined]), so the map verdict must be frozen and callable for either
  precondition to be evaluated.
- The [capture hook][capture] is a second writer and reader of the same cache
  and has the same deny-then-fail shape.
- [rust-mode-transform.test.ts:466][t466] hangs `app.agents()` with a fresh
  session id and asserts `todo_tool_present: true` after about 2 s;
  [hook-handlers.test.ts:119][t119] does the same in the capture hook. Neither
  stores a deny first. [:440][t440] stores a deny through a successful read but
  never fails a later one.
- The only invalidation between passes is
  [`clearToolPermissionDenied`][clearperm] on `session.deleted`
  ([hook.ts:375][hookclear]) and LRU eviction from the 2000-entry
  [map][permmap].

## Failure scenario

Not a violation; a coverage gap. Without a pass that enters the catch with a
stored `true`, P1's fallback clause is satisfied vacuously and a rewrite that
lifts the deny on failure passes the suite.

## Timing windows and dependencies

The two preconditions come from different passes. Pass one must complete a
live read that evaluates to deny and store it; pass two, on the same session,
must reject or exceed 2000 ms before `Promise.all` settles. No invalidation
or eviction may land between them.

## What a test must construct

An SDK fake whose `app.agents()` returns an agent with a whole-tool `deny` on
`todowrite` once and then rejects or never settles, driven through two runs on
one session with a frozen callable map verdict. The marker records, at entry
to [`resolveCombinedTodowriteVerdict`][combined]:
`cachedToolPermissionDenied(sessionId, "todowrite") === true` and that the
read for this pass rejected or timed out. It asserts both preconditions and
not the served value. The
[plugin checks](../existing-checks.md#plugin-pre-send) contain no such
sequence.

## Investigation log

The catalog record lists no open questions. One question was checked while
placing the marker.

### Q: Are both preconditions observable at one code point?

- Sources examined: [`resolveCombinedTodowriteVerdict`][combined], the seed at
  [`:85`][combinedseed], the catch at [`:97-104`][combinedcatch].
- Findings: The seed reads the cache before the `try`, and the catch runs only
  when the read rejected or timed out, so inside the catch both facts are in
  scope: the pre-read cache value and the failure. No other code point sees
  both.
- Missing evidence: None for placement; the campaign itself has not run.
- Conclusion: resolved with answer - the catch body is the marker site, and the
  outcome oracle belongs to P1's test, not to this marker.

[combined]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L77-L107
[combinedseed]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L85-L96
[combinedcatch]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L97-L104
[permdenied]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L277-L308
[permstore]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L306
[permkey]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L71-L73
[permmap]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L57-L59
[clearperm]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L326-L334
[capture]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L270-L292
[hookclear]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.ts#L375
[timeout]: ../../../../../packages/opencode-plugin/src/shared/with-timeout.ts#L2
[t466]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L466
[t440]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L440
[t119]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.test.ts#L119

## Historical first implementation investigation

The following record preserves the first local execution. Its `witness`,
`witness-test`, `resolver`, and `invalidation` relative locators are stale after
the uncommitted single-flight revision. The final investigation has live links.

### Q: Is the exact deny-then-failure situation exercised at the hook seam?

- Investigation: 2026-09-10, working tree on
  `ab2ef4156b69454b407bd9682d5617ad13c8372f`. Earlier narrative and line
  locators remain discovery evidence for the stated `9132344` baseline.
- Sources examined: [hook fixture][witness-test], [constant marker][witness],
  [resolver][resolver], and [invalidation][invalidation].
- User approval provenance: The implementation request requires deny on every
  failed or timed-out read, including an empty cache or expired allow; a
  30-second maximum successful-read freshness; invalidation on session update,
  native compaction, and flush; clearing on deletion. It explicitly requires
  stale deny retention during freshness invalidation. This resolves the
  baseline's fail-open/default and staleness questions, not by inference from
  old code. The full decision is recorded in the P1 investigation log.
- Construction: Drive one successful deny through
  `experimental.chat.messages.transform`. For rejection, deliver
  `session.updated`; for timeout, advance Bun's fake clock 30,000 ms. Read the
  cached verdict before entering each consumer. Start the SDK promise and
  observe its invocation through the mock. Reject it, or leave it pending and
  advance the actual timeout timer 2,000 ms. Await the hook's completion.
- Independent marker: Assert a cached deny at resolver entry AND the live
  SDK read's failure. The test spies on the existing failure log and requires
  an `Error` for rejection or `TimeoutError` for timeout, as well as an SDK
  invocation. The constant string is
  `todowrite-deny-then-read-failure-is-exercised`. No marker name is derived
  from a session, test variant, or outcome. No production test callback is
  added. Both transform and capture independently satisfy the preconditions.
- Separate outcome oracle: Assert every captured transform body has
  `todo_tool_present: false`, no `todo_state.set` is sent, and the stale deny
  remains stored. These outcomes do not decide whether the marker fires.
- Execution: The proof-first run passed the two initial cached-deny variants
  and failed the fresh-hit and unavailable-client cases. The final focused
  six-file campaign passes 162 tests, 635 assertions, including rejection and
  timeout witnesses with the independent failure observation. Commands and
  intermediate failures are retained in the P1 investigation log.
- Missing evidence: This is a controlled hook-fixture campaign, not a claim
  that a production outage occurred. Full repo checks, smoke, and independent
  adequacy review remain with the outer orchestrator.
- Conclusion: resolved with executed evidence. The exact P5 situation is
  constructed, including a cached deny that survives freshness invalidation.

[witness-test]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L162-L239
[witness]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L204-L230
[resolver]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L293-L323
[invalidation]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L382-L397

## Single-flight investigation

### Q: Does the marker remain independent of the fail-closed result?

- Sources examined: [live hook witness][current-witness] and
  [explicit cache peek and invalidation][current-peek].
- Findings: `peekToolPermissionDeniedForTest(sessionId, "todowrite", agent)`
  observes a successful deny before each hook invocation. Freshness
  invalidation or TTL expiry preserves it. The SDK mock then starts a live
  read that rejects or times out. The marker combines the pre-read cache
  observation, SDK invocation, and observed Error/TimeoutError. Assertions on
  the wire body, captured snapshots, and retained deny remain separate.
- Rationale: This is independent situation coverage. A fail-closed empty cache
  returns the same deny, so the marker is not causal evidence that retaining
  a cached deny changes the outcome. The older discussion of the fallback
  branch describes the baseline, not the reason this marker is required.
- Findings: Valid same-key overlap shares one fill and its timeout. Revocation
  cannot be bypassed by an older fill. No fake callback is added to production;
  the test reuses SDK mocks, the existing log spy, fake timers, and the explicit
  test-only peek. Stale last-success state is preserved for this exact witness.
- Execution: The focused six-file campaign passes 169 tests with 675
  assertions, including both P5 variants and real transform/capture overlap
  for build and empty-string agent names. The three proof-first overlap
  failures and supplemental test-type diagnostics are recorded in P1.
- Conclusion: P5 is exercised as an independent cached-deny-plus-read-failure
  witness. It makes no claim that the cached deny changes fallback output.

[current-witness]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L166-L243
[current-peek]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L393-L417
