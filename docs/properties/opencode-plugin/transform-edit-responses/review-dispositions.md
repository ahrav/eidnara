# Source-guard review dispositions

## Scope

These dispositions address the consolidated findings supplied after six
independent reviews of the source-guard precursor. They concern the detached
`533-source-guards` worktree based on `352ce13f`, not the full implementation
snapshot or the primary workspace. Source, tests and this scoped supplement
are the only edited files. No ownership or response-protocol cutover is claimed.

## Verified defects and repairs

1. **Out-of-bounds tape reads: fixed.** `recordOrCompare` checks the tape bound
   before reading a field. A regression installs an inherited numeric getter
   beyond the captured tape, grows the live array and checks rejection with zero
   getter calls. The test restores the descriptor before running any assertions.
   The combined repair made this witness pass. An independent mutation check
   found that the new array terminators also reject this case before the tape
   ends, so the witness does not isolate the explicit bounds check. That check
   remains defense in depth, not an independently demonstrated requirement of
   this fixture.
2. **Array metadata collisions and missing root metadata: fixed.** Array tapes
   encode extra-key markers, names, order, attributes and an explicit end marker.
   Capture retains a separate root tape while recording each member. Recheck
   compares that tape as well as member identity and fields. Tests independently
   describe renamed keys, changed root values/types/descriptors, and extras moved
   between nested arrays. The metadata regression tests fail before the repair.
3. **Hidden own data lookup: fixed.** `readOwnDataProperty` accepts data
   descriptors regardless of enumerability and still rejects accessors and
   proxies. Tests exercise hidden `info` and hidden `output.messages` through the
   actual hook and verify dispatch and the approved result. Array `length` is a
   supported hidden data property. The length-read test fails before the repair.
4. **Telemetry output traversal: removed.** Output count reads only the own
   `messages` and `length` descriptors and checks the result's number type.
   Logging does not perform another recursive output walk.
5. **Wrapper post-hook traversal: narrowed.** A shared root-returnability check
   verifies a non-proxy plain array and its prototype chain before checking for
   `then`. Full source validation still precedes `slice`. Tests install inherited
   `Array.prototype.then` at entry and during the hook await; neither getter runs.
   A separate test proves that the post-hook check does not inspect nested data.
   This check prevents unsafe promise assimilation, not publication already made
   through the host array. PR2 owns publication and rollback behavior.
6. **Silent declines and lost NACKs: fixed.** Existing loggers distinguish
   `SourceRejected` from `SourceWalkLimitExceeded` at entry and continuation
   refusal, with no daemon-failure increment. The outer source-rejection catch
   can reach the existing pending-delivery NACK closure. Earlier disposition
   paths consume that closure to avoid duplicate NACKs. A named microtask-window
   test proves that the pre-await check ran, mutation followed it, and the known
   delivery received one NACK without another transform send or publication.
7. **Redundant validation: narrowly removed.** Direct entry skips separate
   output validation only when `output.messages === liveMessages`; capture
   validates that same tree. Telemetry and wrapper post-hook walks are removed
   as described above. Required source rechecks remain.
8. **Oracle and enabling-state gaps: fixed.** Directory, ordinal, response and
   retry pauses each have an unmutated successful control. Positive controls
   require exact independently specified output and the applied delivery's ACK.
   Delta tests inspect authoritative serialized native input and compare it with
   expected wire data, not a production snapshot function's result.

## Refuted or deferred suggestions

- **Remove checks after `await assertCurrentRetryPass()`: rejected.** The await
  opens a real microtask window even when its synchronous check succeeds. The
  retained post-await deep check and the NACK regression exercise this window.
- **Drop hook-side capture and recapture in direct `run`: rejected as stated,
  then narrowed.** A mutation during directory lookup would become the direct
  call's accepted input instead of being detected against hook entry, so the
  hook's capture stays before that await. The hook now passes that capture to
  `run`, which rechecks against it at its first post-await check instead of
  building a second tape from the same synchronous state. Direct callers that
  supply no capture still capture at entry. PR2 moves ownership before
  preflight without discarding this obligation.
- **Recheck the source on every wire page: removed.** Page bodies are
  serialized before the send loop, so a per-page walk cannot change the bytes
  sent, and its cost grows with page count times history size. The loop keeps
  the cleared/superseded ownership check; the recheck before serialization and
  the one before publication bracket the series, and a paged mutation test
  proves publication is still refused and known deliveries are NACKed.
- **Back-to-back rechecks with no intervening await: removed.** The checks after
  the subagent and prompt-hash lookups and before the availability reads ran
  with no await or host callback since the post-directory check. A regression
  test counts walks between the prompt-hash read and the permission read.
- **Arbitrary nested visitor calls as a public defect: not established.** The
  walker is private and callers cannot supply arbitrary nested operations.
  Preserving the outer visitor is nevertheless necessary for the concrete root
  metadata recording/comparison performed by the repaired capture path. Tests
  cover that reachable use, not a fabricated private API.
- **Negative caches, cheap mutation counters or removing string charges:
  rejected.** None proves recursive equality or the required conservative
  source/snapshot bound. The implementation adds none of them.
- **Treat 64 MiB of JSON as the source-walk admission unit: rejected.** The walk
  counts UTF-16 strings, descriptors, snapshot slots and repeated shared-tree
  visits. It is separate from encoded-output bytes and from aggregate live
  capture accounting. It is not a whole-process RSS guarantee, and JSON below
  the encoded-output limit can still exceed the source-walk estimate. A later
  review measured real sessions against the 64 MiB estimate and found that it
  declined histories of 10 to 18 MiB JSON on every pass; the budget is now
  256 MiB and walk-limit declines log at warn. See
  [the source-guard supplement](source-guards.md#measured-local-cost).
- **Split the walker class or an 87-line method solely for style: deferred.**
  No additional correctness defect requires that restructuring. The repair
  shares one internal record/compare operation without a broader refactor.

## Verification and remaining boundary

The concurrent review set was `ponytail-review`, `reduce-complexity`,
`test-strategy`, `typescript-code-reviewer`, `typescript-design-review`, and
`bounded-design`. The additional TypeScript reviews cover runtime correctness
and the architecture/language boundary; bounded design covers traversal
saturation. A fresh read-only pass verified the repaired source mechanism and
reproduced the 193-test focused run. Its remaining performance concern is
recorded in [the source-guard supplement](source-guards.md#measured-local-cost).

- The four initial regression failures are reproduced before the helper fixes.
- Focused six-file tests pass: 193 tests, 1,441 assertions, zero failures.
- The ten-file Biome check and plugin typecheck pass.
- Root `bun run check:repo` passes, including 3,351 OpenCode tests and builds.
- Plugin smart-note Wasm and TUI import smoke checks pass.
- Node 24.18.0 proxy, descriptor, metadata and accessor probes pass without traps.
- Comment-marker and whitespace checks pass. The 1,000-message sequential
   benchmark's one-sample oracle smoke passes, without a performance comparison.
- Native build, addon tests, host-client tests, payload and tarball smoke,
  fixture-contract checks, mode-manifest checks, and incident validators pass
  in this worktree. The pinned OpenCode 1.18.22 E2E command exits successfully,
  but its 43 Rust cases skip because the runtime reports
  `runtime_mechanism_unavailable`. That result is not transport execution proof.

Admission, cancellation/settlement charges, pass-local ordinals, atomic current
publication, rollback removal and promotion before delivery waits remain PR2
work. Native-addon-enabled CI and broader Rust checks are not replaced by the
root Bun gate. The precursor has a cohesive checked source-guard boundary;
#533 is not complete until its deferred ownership work is complete.
