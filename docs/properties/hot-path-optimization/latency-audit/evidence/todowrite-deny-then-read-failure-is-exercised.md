# todowrite-deny-then-read-failure-is-exercised

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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
