# wp-p11-fail-open-retains-its-acknowledgment-basis

## Discovery trigger

Record WP-P11 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. WP-E11 records raw fallback; PR #819's retained-fold fail-open
is the full-array predecessor. #832 PR two (#884) scopes it to the window
basis.

Exercised status: yes - "appends exactly the unacknowledged window suffix and
promotes no basis" folds, grows the host, fails twice, and asserts the exact
suffix by identity with the boundary unchanged; "serves raw against a
mismatched basis anchor or a changed terminal message" constructs both
refusals; "serves raw when a rerun fails after rediscovering the basis the
daemon disowned" covers the rerun guard. All ran in the #884 `bun run
check:repo` gate (head `f8734c12`).

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #884 (`window-protocol/m1-plugin-fail-open`, head `f8734c12`)
  `bun run check:repo ok`; `test:rust` 23 pass, 17 skip, 0 fail.
- The suffix test folds a five-message host at anchor `m-2`, grows to eight,
  fails, and asserts output length 4: the folded entry followed by
  `grown[5..8]` by identity; `failureCount` 1; `boundary` still `m-2`; a later
  failure appends from the same prefix (length 5).
- The refusal test asserts raw output when the failed pass declared `m-3`
  against a basis at `m-2`, and when `host[4]` (the terminal of the
  acknowledged prefix) was edited in place.
- The transform-edit-responses catalog header records the WP-P11 conditions
  and cites `serveLastApplied` at `rust-mode-transform.ts:1101`, which
  matches the code at `f2442b2f`; its equal-anchor source (`:1393`) and
  disown (`:1645`) citations were stale at `52eda0fa`, and #833 moves them to
  the code at `:1406` and `:1661`.
- #884 Deviations: fail-open also covers `capture_bytes` declines, as #819
  did; #819's "applied output grew" check is dropped. #884 also restores
  #875's `LAST_APPLIED_DECLINES` (`session_busy` and unrecognized statuses)
  under the same rule, and a rerun after `boundary_unknown` never records a
  fail-open source.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #884 (#832 PR two; head `f8734c12`, base
  `window-protocol/m1-plugin-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).

## Failure scenario

Pass one folds at anchor A. The next pass declares anchor B after a summarizer
publication and fails. Reusing A's tapes appends the window after the wrong
prefix, duplicating messages already in B's fold.

## Timing windows and dependencies

Between the applied pass and the failing pass; the terminal message can change
in place in between.

## What a test must construct

Fold once, then fail the next pass under an equal anchor with appended
messages and assert the exact suffix; fail under a different anchor and after
a terminal edit and assert raw output; assert no basis is promoted.

## Investigation log

### Q: Is the exact suffix asserted, not only length?

- Sources examined: `rust-mode-window.test.ts:1031`; #884 Evidence ("exact
  suffix, not length").
- Findings: Each suffix entry is compared by identity with `toBe`.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; the test ran in #884's `bun run
  check:repo` gate.

### Q: Is fallback output bounded?

- Sources examined: Catalog revision 2 open-question resolution.
- Findings: Fallback is never larger than raw for the same window plus the
  retained fold.
- Missing evidence: None.
- Conclusion: resolved with answer by construction.
