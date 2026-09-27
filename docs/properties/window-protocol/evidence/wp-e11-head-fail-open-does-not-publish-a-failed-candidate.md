# wp-e11-head-fail-open-does-not-publish-a-failed-candidate

## Discovery trigger

Record WP-E11 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Catalog revision 2 records HEAD's raw fallback and marks it
invalidated; the transform-edit-responses catalog records WP-E11 as the WP-P11
baseline for #832.

Exercised status: yes - "does not resend after an outcome-unknown transport
failure and recovers on the next attempt" asserts one uncertain send,
unchanged input identity, and a later success; it ran in the #878 `bun run
check:repo` gate (head `f6b3d7bd`) and still runs in the #883 (head
`d7712d75`, `:3298`) and #884 (head `f8734c12`) `bun run check:repo` gates.
#884 replaces the raw-only fallback with WP-P11.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #878 (head `f6b3d7bd`) `bun run check:repo ok`; #877
  also records `check:repo ok`.
- #883 (#832 PR one, head `d7712d75`) serves raw on a real failure; #884 (#832
  PR two, head `f8734c12`) adds window-scoped fail-open. #884's Evidence
  records the baseline: "PR one's failure path serves the input unchanged and
  publishes no failed candidate."
- The transform-edit-responses catalog header at `f2442b2f` states: "WP-E11 in
  #824 is the WP-P11 baseline: a failed pass publishes no failed candidate,
  and the first #832 change served the input unchanged."
- #875's later commit `1431467c` ("Serve the last applied output on a busy or
  unrecognized-status decline") landed on `main` with #875 (merge `0b3465e1`).
  #883 serves those declines raw; #884 restores its `LAST_APPLIED_DECLINES`
  set under the window rule and ports its tests ("serves the last applied
  output plus the appended messages on a ${status} decline",
  `rust-mode-transform.test.ts:3394`). At `f2442b2f` the test is at that line
  and the set is `LAST_APPLIED_DECLINES` (`rust-mode-transform.ts:824`); it
  ran in #833's `bun run check:repo` gate.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #877 (#829 PR A; head `3844a179`): `bun run check:repo` ok (one unrelated
  flaky timer test passed 3 of 3 on rerun), fmt, clippy, `-p daemon`, fixture
  build ok; fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17
  skip, 0 fail.
- #878 (#829 PR B; head `f6b3d7bd`): fmt, clippy, `-p daemon`, `-p
  memory-store`, doctests, markers, `bun run check:repo`, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail.
- #883 (#832 PR one; head `d7712d75`, base
  `window-protocol/m1-daemon-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).
- #884 (#832 PR two; head `f8734c12`, base
  `window-protocol/m1-plugin-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `f2442b2f`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 107, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

The daemon commits, the response is lost, and the plugin publishes a partially
built candidate or resends until the deadline.

## Timing windows and dependencies

Between send and response.

## What a test must construct

Throw from the first transform call after a possible send; assert one call, no
publication, unchanged input identity, and a successful next pass.

## Investigation log

### Q: Which clause survives into WP-P11?

- Sources examined: Comment 3 record; #884 description.
- Findings: No failed candidate is ever published and no basis is promoted;
  the raw-only output is replaced by `applied ++ window[rawCount..]` when the
  basis matches.
- Missing evidence: None.
- Conclusion: resolved with answer: the no-publication clause survives; #884's
  WP-P11 witnesses ran in its `bun run check:repo` gate.

### Q: Is the busy-decline change of `1431467c` in scope?

- Sources examined: #875 commit list via `gh pr view 875 --json commits`.
- Findings: The commit post-dates #875's description and merged with #875.
  #884 restores its declines under the window rule; its ported tests ran in
  #884's `bun run check:repo` gate.
- Missing evidence: None. Re-verified at `f2442b2f`
  (`rust-mode-transform.test.ts:3394`, `rust-mode-transform.ts:824`).
- Conclusion: resolved with answer: in scope and carried by #884.
