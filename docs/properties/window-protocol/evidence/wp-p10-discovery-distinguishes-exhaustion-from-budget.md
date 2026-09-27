# wp-p10-discovery-distinguishes-exhaustion-from-budget

## Discovery trigger

Record WP-P10 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin, protocol. Discovery is new in revision 3. #827 carries the
daemon side; #832 carries the plugin side with WP-P21.

Exercised status: yes - the daemon side (pages newest first, at most 4,096,
never above the rendered row, empty page terminal, a long unlisted run does
not end the walk, malformed and unsafe bodies `invalid_params`) ran in the
#875 daemon gate. The plugin side ran in the #883 (head `d7712d75`) and #884
(head `f8734c12`) `bun run check:repo` gates: "declares a match found past
page one", "sends null with the whole array only after an empty page",
"declines ${name} without sending null" (repeated cursor, ascending,
malformed, unsafe-sequence, timeout, and revision 2 daemon pages), "declines
when the time budget fires with no null submission", and "rediscovers once
after boundary_unknown and declines the second in one pass".

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #875 daemon gate. `transform.boundary` takes `{ v: 3,
  session_id, before_sequence? }` and answers `{ anchors: [{ mid, sequence }]
  }`.
- Wire document: `docs/host-wire-protocol.md` section 7.10.2 (#875) defines
  the grammar, safe integers, and `(mid, sequence)` identity after suffix
  removal.
- #883 (head `d7712d75`): the plugin pages under `DISCOVERY_BUDGET_MS =
  TRANSFORM_SEND_TIMEOUT_MS - 4_000` (`rust-mode-transform.ts:796`), rejects
  pages not strictly decreasing below the cursor, filters by a sorted
  `Uint32Array` of FNV-1a hashes charged to the lease, and verifies hits by id
  scan.
- #883 tests (run in the #883 (head `d7712d75`) and #884 (head `f8734c12`)
  `bun run check:repo` gates): "sends null with the whole array only after an
  empty page" (cursors `[undefined, 5]`, `boundary` null); "declines ${name}
  without sending null" for a repeated cursor, an ascending page, a malformed
  page, an unsafe sequence, a timeout, and a revision 2 daemon (no body,
  `boundary` undefined, `failureCount` 0).
- Review P6 on #832: the budget must be anchored at the pass start, not
  discovery start; #883 records "budget from pass start, rerun budget
  documented and tested" ("declines a rediscovery once a slow first send spent
  the pass's discovery budget", `rust-mode-window.test.ts:754`). Review P1: a
  `boundary_unknown` reran a second discovery; #883 first recorded "one
  discovery per pass" with "declines boundary_unknown after discovering a
  stored anchor the host lost". #883's review update (`adcc7baf`, merged)
  replaced it: the first `boundary_unknown` now rediscovers once, witnessed by
  "rediscovers and publishes when an anchor it just discovered draws
  boundary_unknown" (`:647`).

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
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
- #883 review update (head `adcc7baf`, merged as `b93059b9`): `bun run
  check:repo` ok, markers ok, `test:rust` 23 pass, 19 skip, 0 fail.
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `3ebfc3b9`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 108, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

Discovery times out on page three of a 60,000-anchor walk and the plugin sends
`boundary: null`. After #833 the daemon resets the session because no anchor
survives in the window.

## Timing windows and dependencies

The walk spans several round trips under the pass deadline; anchors change
only at the newest end (D16).

## What a test must construct

Serve pages that match on page two, pages that end empty, pages with repeated
or ascending cursors, malformed or unsafe entries, a timeout, and a revision 2
answer; assert null only after the empty page and a local decline otherwise.

## Investigation log

### Q: Is the budget anchored at the pass start?

- Sources examined: #832 review P6; #883 description;
  `rust-mode-window.test.ts:754`.
- Findings: Yes at `d7712d75`: #883 measures the budget from the pass start,
  and a slow first send that spent it declines the rediscovery.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; the test ran in the #883 and #884
  `bun run check:repo` gates.

### Q: Does a long run of unlisted rows end the walk early?

- Sources examined: `a_long_run_of_unlisted_rows_does_not_end_the_walk` (4,499
  rows).
- Findings: No; unlisted rows are filtered in SQL before `LIMIT`.
- Missing evidence: None.
- Conclusion: resolved with answer.
