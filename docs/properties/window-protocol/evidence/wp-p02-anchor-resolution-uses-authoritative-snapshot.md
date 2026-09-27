# wp-p02-anchor-resolution-uses-authoritative-snapshot

## Discovery trigger

Record WP-P02 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Revision 3 moves coverage interpretation to the daemon. Ticket
#827 carries the resolver with one witness per transition row and the snapshot
clause.

Exercised status: yes -
`each_resolution_outcome_has_its_cut_ordinals_and_keeps` covers Normal,
StaleSlice, Revert with and without `keep_through_seq`, Unknown, and
FirstPass;
`a_publish_between_the_core_read_and_the_declared_read_is_invisible` and
`a_removal_between_the_core_read_and_the_intersection_is_invisible` hold a
barrier in the snapshot; all ran in the #875 daemon gate. At the handler,
#881's `each_resolution_outcome_runs_through_the_handler_with_its_effects`
runs every outcome with its effects (`boundary_unknown` leaves `row_version`,
meta, and segments unchanged), and
`a_publish_during_a_null_boundary_pass_keeps_the_cut_at_the_rendered_boundary`
and
`a_reset_that_moves_the_cut_leaves_the_served_window_in_the_ready_snapshot`
change the snapshot between resolution and a CAS attempt; they ran in the #881
`cargo test -p daemon` gate (head `1c66f16c`).

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #875 `cargo test -p daemon --all-features --locked ok
  (2,766)`; commits `7e8e6ec0`, `dd5f9d1a`, `45fa9843` carry the module.
- The table test names seven cases, including "no boundary, coverage held, and
  no surviving segment end" resolving `Revert { keep_through_seq: None }`, and
  asserts resolution, ordinals, window length minus cut, and the keep start.
- The barrier tests install `store.set_coverage_snapshot_hook` to commit a
  publish or a removal inside the read transaction and assert the held
  resolution equals the pre-change snapshot (`Normal` with ordinals `[10,
  11]`; `StaleSlice { cut: 7 }`), while a fresh resolution sees the change.
- `a_declared_row_without_a_rendered_boundary_is_unknown`: reachable after a
  committed truncate whose pass then failed (#875 description).
- #881 (head `1c66f16c`): the handler resolves once in `start_transform_pass`
  (`crates/daemon/src/lib.rs:8683`, the call at `:8699`) and again from a
  fresh snapshot on each CAS attempt; the pass's own load must have the same
  `row_version`. #881 also bounds the null-boundary stale-slice lookup to rows
  at or below the rendered boundary, a refinement of D10 its description
  records; `a_null_anchor_hit_above_the_rendered_row_is_not_cut_at` fails
  without the bound.
- Review F4 on #831: the outer request kept the first cut while attempts
  re-resolved. #881 folds the served window into `served_request`
  (`crates/daemon/src/transform.rs:1540`), which the summarizer, native
  attach, ready snapshot, and response read (`crates/daemon/src/lib.rs:8968`,
  `:9015`); the two handler tests above assert the ready snapshot holds the
  served window.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `test -p
  daemon` ok (one run hit a known timing flake in a 33-test binary; the rerun
  was green), `-p memory-store`, `check -p storage --no-default-features`, `-p
  host-runtime --test protocol_vectors`, doctests, markers, `bun run
  check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin
  is refused) and 23 pass, 17 skip, 0 fail with #883.

## Failure scenario

The resolver reads core, then a summarizer publishes a newer segment, then the
resolver reads the declared row. It concludes the declared anchor is older and
the rendered mid is absent, and truncates history the host still holds.

## Timing windows and dependencies

Inside the resolver's reads. A single read transaction closes the window; the
barrier test commits inside it.

## What a test must construct

Seed segments for each outcome; call `resolve` with a declared anchor per row;
install a snapshot hook that commits a publish or a removal between reads;
assert the resolution equals the pre-change snapshot.

## Investigation log

### Q: Can `Revert(None)` be reported as `Unknown`?

- Sources examined: Table test at `tests.rs:99`.
- Findings: The null-anchor, coverage-held, no-survivor case asserts `Revert {
  keep_through_seq: None }`.
- Missing evidence: None.
- Conclusion: resolved with answer: no.

### Q: Do outer consumers use the attempt's resolution?

- Sources examined: #831 review F4; #881 description; `revision_3.rs:876`,
  `:917`.
- Findings: Yes at `1c66f16c`: `served_request` carries the attempt's final
  window to every outer consumer.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; the witnesses ran in #881's daemon
  gate.
