# wp-p14-first-user-policy-comes-from-session-authority

## Discovery trigger

Record WP-P14 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. A window can start after the session's first user message.
#832 PR two (#884) routes policy to the session database.

Exercised status: partial - verdict direction constructed; frozen status
unasserted. "takes the verdict from the earliest user row in both
signal directions" and "freezes fail-open without a database and stays
fail-closed for an unpersisted session" run cold passes whose window starts
past the first user against an installed database;
`eidnara-reduce-availability.test.ts` "keeps separate eidnara_reduce and
todowrite verdicts for one session" pins the per-tool cache key. All ran in
the #884 `bun run check:repo` gate (head `f8734c12`).

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #884 (head `f8734c12`) `bun run check:repo ok`. The test
  installs an OpenCode database under `XDG_DATA_HOME` whose earliest user row
  carries `tools`, runs a cold pass whose window starts at `m-3`, and reads
  `tool_present` and `todo_tool_present` from the request body.
- Deny at the earliest row with allow at the window user yields `[false,
  false]`; the reverse yields `[true, true]`.
- No database yields `[true, true]`; a database with no row for the session
  yields `[false, false]`.
- The first-user lookup is index-bounded on OpenCode's
  `message_session_time_created_id_idx` (catalog revision 2, WP-P07 inventory;
  0.02 ms warm on a 2,054-message session in a 110,006-message database,
  measured for the specification).

Gate results of the runs cited here, as recorded in the PR descriptions:

- #884 (#832 PR two; head `f8734c12`, base
  `window-protocol/m1-plugin-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).

## Failure scenario

The session's first user message denied `todowrite`; the window's first user
allows it. After a plugin restart a window-based resolver unfreezes the tool.

## Timing windows and dependencies

Only on a cache miss: first pass after restart.

## What a test must construct

Install a database with contradictory earliest-user tools, run a cold pass
with a window past that message, and compare the verdict with the database
row; repeat without a database and without a row.

## Investigation log

### Q: Is the frozen status compared, not only the Boolean?

- Sources examined: `rust-mode-window.test.ts:1293`.
- Findings: The test name states freeze behavior; the assertions compare the
  request's tool flags.
- Missing evidence: A direct assertion on the frozen flag; #884's tests
  compare `tool_present` and `todo_tool_present` only.
- Conclusion: unresolved, needs a frozen-status assertion. Queued in
  `portfolio-evaluation.md`.

### Q: Is window-first-user seeding still reachable?

- Sources examined: #832 review Q3; D23.
- Findings: No. #884 deletes `resolveEidnaraReduceAvailabilityFromMessages`,
  `resolveTodowriteAvailabilityFromMessages`, and
  `resolveToolAvailabilityFromMessages`; none has a `git grep` hit in
  `packages` at `f2442b2f`.
- Missing evidence: None.
- Conclusion: resolved with answer: not reachable; the resolvers are deleted.
