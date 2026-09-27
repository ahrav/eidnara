# wp-e02-recut-keeps-the-surviving-prefix

## Discovery trigger

Record WP-E02 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Catalog revision 2 records the surviving-prefix recut as
existing behavior. Revision 3 replaces the surviving-prefix scan with the
resolver's `keep_through_seq`, so the success oracle must survive the change.

Exercised status: yes -
`reconcile_rematerialize_with_unrecut_store_truncates_and_refolds_prefix`
removes a later anchor with an earlier one surviving and asserts the HARD
refold, `coverage_ordinal` 1, and a cleared `reconcile_pending`; it ran in the
#880 daemon gate (head `748c0c4b`) and in the #881 `cargo test -p daemon` gate
(head `1c66f16c`). #881 adds
`a_cas_conflict_after_the_revert_truncate_folds_in_the_same_request` (the
interrupted request answers the HARD against the surviving anchor,
`revert_epoch` + 1, one segment) and
`a_panic_plus_reopen_after_the_revert_truncate_folds_on_the_next_pass`
(discovery lists the surviving anchor after reopen and the next pass folds),
both of which also re-enter the truncate and assert a no-op; both ran in the
same gate.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #880 (head `748c0c4b`) `cargo test -p daemon ok`; the
  witness is at `transform.rs:20369` there and `:19455` at `f2442b2f`.
- The witness asserts the revert pass is `SOFT+` with `reconcile_pending`,
  then a HARD whose `coverage_ordinal` is `Some(1)` with `reconcile_pending`
  false.
- Store primitive: `truncate_history_segments_for_revert`
  (`crates/memory-store/src/lib.rs:12001` at `f2442b2f`); its test at `:26148`
  asserts the first cut bumps `revert_epoch` to 1 and `row_version` by one,
  and a repeated cut is a no-op on both.
- #881 (head `1c66f16c`): `Revert { Some }` enters the reconcile path with the
  truncate keeping history through the resolved anchor's sequence;
  `surviving_revert_prefix_seq` is deleted (listed among 0-hit deletions). The
  interrupted-revert tests are `revision_3.rs:1147` and `:1163`; their shared
  setup `interrupted_revert` (`:1082`) runs the interruption from
  `install_transform_attempt_hook` after asserting the truncate committed (one
  segment left).
- Review finding F1 on #831 (blocking in round 1): the in-request CAS retry
  resolved `Unknown` after a committed truncate. #881 lists the fix as
  accepted ("interrupted revert follows D10 in the same request and after
  panic plus reopen"): `rendered_coverage_tx` treats the newest surviving
  segment as the rendered boundary while `reconcile_pending` is set and the
  `core.boundary_id` row is gone, and `assert_folded` (`revision_3.rs:1120`)
  asserts the interrupted request's own HARD.
- The #874 inventory drives a HARD with an injected CAS conflict
  (`every_pass_read_is_bounded_independent_of_history_size`,
  `crates/daemon/src/transform_read_bound.rs:627` at `3ebfc3b9`), which is
  read-bound evidence, not recut evidence.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #874 (#826 PR 2; description head `704568ec`, head at read `c239461c`): fmt,
  clippy, `-p memory-store`, `-p daemon`, doctests, storage no-default check,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  0 fail, 17 skip.
- #880 (#830 PR 2; head `748c0c4b`): fmt, clippy, `-p daemon`, doctests,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  23 pass, 17 skip, 0 fail.
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `test -p
  daemon` ok (one run hit a known timing flake in a 33-test binary; the rerun
  was green), `-p memory-store`, `check -p storage --no-default-features`, `-p
  host-runtime --test protocol_vectors`, doctests, markers, `bun run
  check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin
  is refused) and 23 pass, 17 skip, 0 fail with #883.

## Failure scenario

A revert removes the newest anchor. A cut at the wrong sequence either deletes
a surviving summary, so its messages return as raw tail, or keeps the reverted
summary, so the model sees removed turns as history.

## Timing windows and dependencies

The truncate commits in its own transaction before the terminal CAS. A CAS
conflict or crash between them leaves the truncate committed and core
unchanged; the next attempt must converge.

## What a test must construct

Seed two segments, commit a HARD, remove the later anchor, and run the HARD
reconcile. Assert the remaining segment set, the new anchor, `revert_epoch` +
1, and the served tail. Then inject a CAS conflict and a panic after the
truncate commit and assert one HARD, one epoch bump, and a no-op re-entered
truncate.

## Investigation log

### Q: Does the interrupted revert converge in the same request?

- Sources examined: #881 description (Interrupted revert, D10);
  `revision_3.rs:1147`, `:1163`.
- Findings: Before fix F1 the retry resolved `Unknown`. At `1c66f16c` a CAS
  conflict after the truncate folds in the same request (the retry resolves
  against the newest surviving segment as the rendered boundary), and a panic
  plus reopen folds on the next pass with no `boundary_unknown`.
- Missing evidence: None.
- Conclusion: resolved with answer: yes for a CAS conflict (the same request
  answers the HARD); after a panic plus reopen the next pass folds. Both ran
  in #881's daemon gate.

### Q: Is the success oracle unchanged by revision 3?

- Sources examined: `transform.rs:19455` at `f2442b2f`.
- Findings: The witness survives on the revision 3 tree through the test-only
  `plugin_window` helper; #881 lists WP-E02 as "existing checks green".
- Missing evidence: None.
- Conclusion: resolved with answer: the oracle is kept and ran in #881's
  daemon gate.
