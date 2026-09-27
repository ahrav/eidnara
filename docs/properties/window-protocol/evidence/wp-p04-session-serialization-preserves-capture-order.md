# wp-p04-session-serialization-preserves-capture-order

## Discovery trigger

Record WP-P04 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Snapshot generation fencing (WP-E08) orders completion, not
execution. Ticket #827 adds the lane.

Exercised status: yes -
`session_lane_holds_one_waiter_refuses_a_third_and_runs_in_arrival_order`,
`session_lane_activates_in_join_order_under_contention` (400 tasks, 4
threads), the three hand-off tests, and
`cancelled_pass_keeps_its_lane_place_until_its_blocked_unit_finishes` ran in
the #875 daemon gate.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #875 daemon gate; lane commits `714d126e`, `67b9631e`,
  `1494470b`, `328ab11c`.
- The lane is a `HashMap<session, Option<oneshot::Sender>>` under one mutex;
  joining and hand-off are atomic, so passes activate in join order (#875
  description).
- The first test asserts one submission while the first pass is held, three
  unit permits available, the third answer `session_busy` with no `operations`
  and an unchanged `durable_state`, then two commits in arrival order and an
  empty lane table.
- The place rides in `PassHold` inside `PassEnv` and is released only after
  the last unit, including Emergency95 reruns and settlement, finishes.
- Plugin: `session_busy` declines as `daemon_session_busy`, any other
  unrecognized status as `daemon_status_unrecognized`
  (`rust-mode-transform.test.ts:437-441`). Later commit `1431467c` serves the
  last applied output on these declines; it merged with #875, #883 serves them
  raw, and #884 restores it under the window rule
  (`rust-mode-transform.test.ts:3394`). At `f2442b2f` the test is at that line
  and the set is `LAST_APPLIED_DECLINES` (`rust-mode-transform.ts:824`); it
  ran in #833's `bun run check:repo` gate.
- Detached work (summarizer publication) is outside the lane and fenced by
  WP-E04.

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
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `3ebfc3b9`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 108, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

Capture A (older) stalls in transport; capture B (newer, host reverted a
message) runs first and folds. A then runs and the daemon reads B's missing
anchor as a revert of A.

## Timing windows and dependencies

Between admission and execution of two same-session passes.

## What a test must construct

Hold the active pass at a barrier; admit a second and a third same-session
request; assert the third is `session_busy` with no durable change, the second
waits and later runs, and commits land in arrival order; repeat under
multi-thread contention.

## Investigation log

### Q: Does cancellation release the lane while a unit can still commit?

- Sources examined:
  `cancelled_pass_keeps_its_lane_place_until_its_blocked_unit_finishes`.
- Findings: No; the place is held until the blocked unit finishes.
- Missing evidence: None.
- Conclusion: resolved with answer: no.

### Q: Does the plugin handle unknown statuses?

- Sources examined: #875 wire document section 7.10.1; plugin test at `:441`.
- Findings: The status set is open; consumers MUST treat an unknown status as
  a declined pass.
- Missing evidence: None.
- Conclusion: resolved with answer: declined without publication.
