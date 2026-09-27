# wp-e03-no-survivor-arms-pending-without-truncation

## Discovery trigger

Record WP-E03 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Catalog revision 2 records the no-survivor pass-through as
HEAD behavior and D10 replaces it for `boundary: null` with an automatic
reset. Ticket #833 carries the disposition: the record stays for
`lineage_switched`.

Exercised status: yes - #833 moves every pre-disposition `pending_rewrite`
test onto a lineage switch (`switched()`), so each reaches the arm only with
`lineage_switched`, and adds the reset contrast
`a_revert_before_the_first_anchor_resets_and_serves_the_window_as_a_first_pass`;
all ran in #833's `cargo test -p daemon` gate (2,749 passed).

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Pre-disposition run: #880 (head `748c0c4b`) `cargo test -p daemon ok` with
  the three witnesses at `transform.rs:20472`, `:20627`, `:20690`; at
  `f2442b2f` they are `:19785`, `:19950`, `:20017`.
- `reconcile_recut_nothing_survives_arms_pending_raw_without_truncate` asserts
  `PASSTHROUGH`, one committed alarm row, an unchanged `core.boundary_id`, and
  `reconcile_pending` false.
- `pending_rewrite_passes_isolate_ingress_meta_usage_and_reconcile` asserts
  an unchanged `meta.last_usage`; `pending_rewrite_persists_across_store_restart`
  asserts the alarm survives reopen and a repeat does not commit.
- #881 (head `1c66f16c`): `Revert { None }` kept the pass-through;
  `each_resolution_outcome_runs_through_the_handler_with_its_effects`
  (`crates/daemon/src/transform/revision_3.rs:323`) asserted `PASSTHROUGH`.
  Outside the arm (a lineage anchor and continuation base present),
  `a_revert_through_no_anchor_outside_the_pending_rewrite_arm_removes_every_segment`
  (`:824`) removes every segment; `:844` and `:859` repeat it under a CAS
  conflict and a panic plus reopen. At `f2442b2f` those three send
  `lineage_switched`, so they stay on the lineage path.
- #833 (D10): the retry loop resets when the resolution is `NO_SURVIVOR` and
  the request has no `lineage_switched` (`crates/daemon/src/transform.rs:1959`);
  `reset_no_survivor` (`:2038`) reads the segment range, runs
  `reset_session_for_recomp` under the row version the resolution read, and
  logs "revert before the first anchor reset ... removed history_segment
  sequences {range}"; the loop then resolves again as `FirstPass`. A second
  no-survivor resolution in the same pass fails with a CAS conflict. The arm
  itself requires `req.lineage_switched` (`:3318-3319`). The Revert-None row
  of `each_resolution_outcome_runs_through_the_handler_with_its_effects` now
  asserts the reset ("the session is reset and the window served as a first
  pass", `revision_3.rs:422`).
- #833 witnesses (`revision_3.rs`): `:447` asserts `Revert { None }` and
  `removed_sequence_range` "1..=2" before the pass, then HARD, `boundary`
  null, `revert_epoch + 1`, no `pending_rewrite`, no segments, and a next
  fold resolving `FirstPass` with ordinals `[1, 2, 3]`. `:498` (one CAS
  conflict, one reset), `:610` (a conflict every attempt: `transform_failed`,
  segments kept), `:635` (a committed reset spends no retry), and `:657` (no
  second reset in one pass) bound the retries; `:701` covers WP-E12. The
  emitted log line is not asserted; `removed_sequence_range` is.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #880 (#830 PR 2; head `748c0c4b`): fmt, clippy, `-p daemon`, doctests,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  23 pass, 17 skip, 0 fail.
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `-p
  daemon` ok (rerun after a known timing flake), `-p memory-store`, storage
  no-default check, `-p host-runtime --test protocol_vectors`, doctests,
  markers, `check:repo`, fixture build ok; fixture-contract 6; manifest ok;
  `test:rust` 9 pass, 14 fail alone (by design), 23 pass, 17 skip with #883.
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `f2442b2f`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 107, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

An unrelated array arrives under a known session id. If the daemon adopted its
identities or truncated history, the session's summaries would be destroyed
for a host that has not reverted.

## Timing windows and dependencies

None within a pass. Across passes the alarm persists through store reopen
until the old anchor returns or `session.recomp` runs.

## What a test must construct

For the kept scope, send a window with `lineage_switched` and no surviving
anchor over a covered session; assert pass-through, an armed alarm, unchanged
history, core, and epoch, and a no-write repeat. For the reset contrast, send
`boundary: null` with no survivor and no `lineage_switched`; assert the reset,
the logged range, and a `FirstPass` response in the same request.

## Investigation log

### Q: Which production inputs still reach the pass-through after D10?

- Sources examined: #833 ticket Scope ("the pass-through `pending_rewrite` arm
  stays reachable only for `lineage_switched`"); `transform.rs:1959`,
  `:3318-3319` at `f2442b2f`.
- Findings: Only a `NO_SURVIVOR` resolution with `lineage_switched` and no
  `anchor_block_id` reaches the arm; without `lineage_switched` the retry loop
  resets before the arm is evaluated.
- Missing evidence: None.
- Conclusion: resolved with answer: the arm is reached only with
  `lineage_switched`; every arm witness sends it.

### Q: Does an identical repeat write?

- Sources examined: `transform.rs:20017` and `:19785` (the restart and
  arming tests).
- Findings: Both assert the repeat does not commit and the `row_version` is
  unchanged; they ran in #833's daemon gate on `switched()` requests.
- Missing evidence: None.
- Conclusion: resolved with answer: no write.
