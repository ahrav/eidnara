# wp-p18-a-waiting-pass-actually-waited

## Discovery trigger

Record WP-P18 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Ticket #827 pairs WP-P04 with this marker.

Exercised status: yes -
`session_lane_holds_one_waiter_refuses_a_third_and_runs_in_arrival_order`
holds the active pass at a barrier, observes one submission and three free
unit permits while the second waits, refuses the third with `session_busy`,
then observes both commits in arrival order; it ran in the #875 daemon gate.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Assertions: `runner.submitted` 1 while held;
  `transform_units.available_permits()` 3; third answer `status`
  `session_busy`, no `operations`, `durable_state` unchanged;
  `submitted_at_first_commit` 1 and `submitted` 2 after; both `committed`; the
  second `row_version` above the first; the lane table empty.
- Recorded run: #875 `cargo test -p daemon ok (2,766)`.
- `session_lane_activates_in_join_order_under_contention` runs 400 tasks on 4
  threads and asserts activation equals join order; #875 records it failed 1
  of 3 runs against the earlier semaphore design, so it discriminates.
- Hand-off controls:
  `session_lane_waiter_dropped_after_hand_off_releases_the_lane`
  (`transform_unit/tests.rs:1202`),
  `session_lane_handed_off_waiter_dropped_unpolled_wakes_the_next` (`:1214`),
  `session_lane_waiter_leaving_before_hand_off_frees_the_slot` (`:1226`).
- `emergency_cancellation_between_units_preserves_commit_and_releases_scratch`
  (`:791`) parks a waiter through the gated settle unit (#875 description).
- The lane (`SessionLanes`, `crates/daemon/src/transform_unit.rs:211`) is
  released from `PassHold` (`:144`) after the last unit.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.

## Failure scenario

Not applicable to a marker; see WP-P04.

## Timing windows and dependencies

While the barrier holds the active pass.

## What a test must construct

Hold the active pass; admit a waiting pass and a third; assert counts and
order.

## Investigation log

### Q: Does the waiter hold a unit permit or a store connection?

- Sources examined: #875 description; permit assertion.
- Findings: No; three of four unit permits remain available while it waits.
- Missing evidence: None.
- Conclusion: resolved with answer.

### Q: Is the marker independent of the WP-P04 safety assertion?

- Sources examined: `transform_unit/tests.rs:1016-1080`.
- Findings: The marker observations (one submission while held, the busy
  answer while two are live) are asserted before the commit-order assertion;
  they fire on a correct lane.
- Missing evidence: None.
- Conclusion: resolved with answer.
