# wp-p09-shrink-first-has-an-explicit-failure-state

## Discovery trigger

Record WP-P09 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. D21 replaces the O(N) all-slot preflight (260 to 300 ms at 1M
slots, twice per pass) with shrink-first. #829's plugin slice (#877)
implements it.

Exercised status: yes - "shrinks first and leaves exactly the candidate in the
original array object" (define order `length`, `0`, `1`), "restores the
captured references after a shrink stopped by a planted non-configurable
slot", "reports a throw while restoring the captured references", "leaves
exactly a shorter candidate in the original array object", and "restores the
captured references and promotes nothing when a planted slot stops the shrink"
ran in the #877 `bun run check:repo` gate. #883 adds the window variant
"appends the captured window after a shrink stopped at a covered|window slot"
("a covered" and "a window" slot, `boundaryIndex` = `covered.length`); it ran
in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run check:repo`
gates.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #877 (head `3844a179`) `bun run check:repo ok`.
- `publishInPlace(target, next, window, boundaryIndex)` defines `length`
  first; on a throw it appends the captured window from index `max(0,
  shrunkLength - boundaryIndex)` and returns `{ error, shrunkLength, detail
  }`; the pass logs one `publication_failed` warning.
- The planted-slot test asserts the `TypeError`, `shrunkLength` k + 1, the
  observed length k + 1 before restoration, detail text naming the restored
  count, and `target` equal to the original members with no candidate entry.
- The pass-level test asserts the same array object, per-slot identity, one
  warn line, unchanged memo, `transform.nack`, and no
  `previous_output_revision` on the next pass (#877 description).
- #883 (head `d7712d75`): the window variant "appends the captured window
  after a shrink stopped at a covered|window slot" passes `covered.length` as
  `boundaryIndex`; it ran in the #883 (head `d7712d75`) and #884 (head
  `f8734c12`) `bun run check:repo` gates.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #877 (#829 PR A; head `3844a179`): `bun run check:repo` ok (one unrelated
  flaky timer test passed 3 of 3 on rerun), fmt, clippy, `-p daemon`, fixture
  build ok; fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17
  skip, 0 fail.
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

The host array has a non-configurable slot at index 3 below the window.
Shrinking to the candidate length deletes slots above 3 and throws. Writing
candidate slots first would leave a mix of candidate and host entries.

## Timing windows and dependencies

The synchronous publication section.

## What a test must construct

A real Bun array with a planted non-configurable slot at k with S <= k;
publish a shorter candidate; assert the throw, length k + 1, zero candidate
writes, restored window references, and no promotion (WP-P20).

## Investigation log

### Q: Is k inside the window covered?

- Sources examined: `transform-capture.test.ts:1138` at `f2442b2f`.
- Findings: The `it.each` names both a covered and a window slot.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; it ran in the #883 and #884 `bun run
  check:repo` gates.

### Q: Can restoration itself throw?

- Sources examined: `publishInPlace`; test `:1104`.
- Findings: A throw while restoring is reported as the same failed
  publication.
- Missing evidence: None.
- Conclusion: resolved with answer.
