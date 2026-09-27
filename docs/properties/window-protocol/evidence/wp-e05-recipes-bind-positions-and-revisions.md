# wp-e05-recipes-bind-positions-and-revisions

## Discovery trigger

Record WP-E05 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Catalog revision 2 records recipe binding as existing
behavior. #830 replaces the native attachment cache with a revision-bound
store and must keep revision binding; #832 moves input keeps to window
coordinates.

Exercised status: yes - the shared edit-recipe fixtures, the retained-output
mutation family, and `native_previous_keeps_bind_the_applied_revision` (stale
revision gets literals; a same-length edit under a matching revision serves as
a cold encode) ran in the #879 daemon gate and the #877 and #878 `bun run
check:repo` gates.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound runs: #879 (head `7a3db43b`) "Edit-recipe fixtures and
  generated single-fault mutations green (WP-E05)" with `cargo test -p daemon
  ok`; #877 (head `3844a179`) and #878 (head `f6b3d7bd`) record `bun run
  check:repo ok`, which runs the plugin suite (`package.json` `check:repo`
  calls `bun run test`).
- `NativeOutputStore::remove` with `RetainedNativeOutput::into_previous`
  (`crates/daemon/src/lib.rs:2317`, `f2442b2f`) returns the previous output
  only when both `revert_epoch` and `previous_output_revision` match;
  otherwise the recipe uses literals.
- `native_previous_keeps_bind_the_applied_revision` asserts a matching
  revision keeps, a stale revision gets literals with no
  `previous_output_revision`, and a same-length edit under a matching revision
  is served as a cold encode would serve it.
- Plugin: the `rejects mutated retained output` family mutates retained output
  before the request and during the response wait and asserts rejection.
- #883 and #884: "sends the declared window in both representations and
  publishes the recipe at boundaryIndex + i" (`rust-mode-window.test.ts:182`,
  `f2442b2f`; `:161` at `d7712d75`) asserts output position i is host index 6
  + i; it ran in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun
  run check:repo` gates. #881's
  `stale_slice_input_keeps_address_the_submitted_native_window`
  (`crates/daemon/src/transform/revision_3.rs:988`) asserts every input keep
  starts inside the submitted window after a stale cut.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #877 (#829 PR A; head `3844a179`): `bun run check:repo` ok (one unrelated
  flaky timer test passed 3 of 3 on rerun), fmt, clippy, `-p daemon`, fixture
  build ok; fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17
  skip, 0 fail.
- #878 (#829 PR B; head `f6b3d7bd`): fmt, clippy, `-p daemon`, `-p
  memory-store`, doctests, markers, `bun run check:repo`, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail.
- #879 (#830 PR 1; head `7a3db43b`): fmt, clippy, `-p daemon`, doctests,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  23 pass, 17 skip, 0 fail (steady-state byte identity included).
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `test -p
  daemon` ok (one run hit a known timing flake in a 33-test binary; the rerun
  was green), `-p memory-store`, `check -p storage --no-default-features`, `-p
  host-runtime --test protocol_vectors`, doctests, markers, `bun run
  check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin
  is refused) and 23 pass, 17 skip, 0 fail with #883.
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

## Failure scenario

The daemon names `keep(previous, 3, 2)` against a revision the plugin
replaced. Without the revision check the plugin splices entries from a
different output into the prompt.

## Timing windows and dependencies

The retained output can be mutated between publication and the next request,
or during the response wait.

## What a test must construct

Publish an output, mutate a retained entry or its revision, then apply a
recipe that keeps from it; assert rejection and unchanged sources. Apply a
recipe with nontrivial input and previous ranges and assert exact
reconstruction and object identity.

## Investigation log

### Q: Does the #830 store keep revision binding?

- Sources examined: #879 description;
  `native_previous_keeps_bind_the_applied_revision`.
- Findings: The store returns the entry only on equal epoch and revision and
  always removes it on `remove`.
- Missing evidence: None.
- Conclusion: resolved with answer: yes.

### Q: Are input keeps in submitted-window coordinates under revision 3?

- Sources examined: #881 description ("input keeps are already in submitted
  coordinates"), #883 window test.
- Findings: The daemon builds the native recipe against the full submitted
  `native_messages`, so keeps are already in submitted coordinates; the plugin
  publishes at `boundaryIndex + i`.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; the daemon side ran in #881's daemon
  gate and the plugin side in the #883 and #884 `bun run check:repo` gates.
