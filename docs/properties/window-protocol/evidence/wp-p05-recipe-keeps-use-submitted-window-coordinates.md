# wp-p05-recipe-keeps-use-submitted-window-coordinates

## Discovery trigger

Record WP-P05 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Stale slicing cuts the submitted window inside the daemon.
Ticket #827 pairs WP-P05 with WP-P19.

Exercised status: yes -
`a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input`
builds keeps at a stale cut of four and reconstructs the served array from the
unsliced input; it ran in the #875 daemon gate. At the handler, #881's
`stale_slice_input_keeps_address_the_submitted_native_window` resolves
`StaleSlice { cut: 2 }` over a native window with no previous output and
asserts every input keep starts at or after the cut and the served native
messages are `m5`, `m6`; it ran in the #881 `cargo test -p daemon` gate (head
`1c66f16c`).

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #875 daemon gate. The test doc reads: "WP-P05 and WP-P19: at a
  stale cut of four, input keeps built against the submitted window start at
  the cut and reconstruct the served array from the unsliced input."
- #881 (head `1c66f16c`): "The native recipe is built against the full
  submitted `native_messages`, so input keeps are already in submitted
  coordinates and `translate_input_keeps` (#827) is deleted." It has no `git
  grep` hit in `crates` at `1c66f16c`.
- #881's Evidence table names
  `stale_slice_input_keeps_address_the_submitted_native_window` for WP-P05;
  its daemon gate is green on `1c66f16c`, which sits on `main` after #880.
- #883 (head `d7712d75`): recipes apply at `boundaryIndex + i` (D20); the
  steady-state window test asserts it and ran in the #883 (head `d7712d75`)
  and #884 (head `f8734c12`) `bun run check:repo` gates.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
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

The plugin submits window `[a, b, c, d, e]`; the daemon cuts two covered
messages and returns `keep(input, 0, 1)` meaning `c`. The plugin keeps `a`.

## Timing windows and dependencies

A fold between two plugin passes moves rendered coverage past the plugin's
declared anchor.

## What a test must construct

Publish one segment after the plugin's last response so the next pass resolves
`StaleSlice { cut > 0 }`; return a recipe with an input keep; reconstruct
against the unsliced submitted window and compare with the served array.

## Investigation log

### Q: Are keeps translated or built in submitted coordinates?

- Sources examined: #881 description; `revision_3.rs:988`.
- Findings: Built against the submitted native window; no translation step
  remains.
- Missing evidence: None.
- Conclusion: resolved with answer: built in submitted coordinates; the test
  ran in #881's daemon gate.

### Q: Does the CK recipe path use the same coordinates?

- Sources examined: #881 description and Evidence table; #879 description
  ("The CK `previous_output` path and the recipe format are unchanged").
- Findings: #881 addresses the native recipe only; its Evidence table names no
  CK keep witness at a nonzero cut, and
  `stale_slice_input_keeps_address_the_submitted_native_window` builds a
  native body.
- Missing evidence: A handler-level CK keep witness at a nonzero cut.
- Conclusion: unresolved, needs a CK keep witness at a nonzero cut; #881 names
  none. Queued in `portfolio-evaluation.md`.
