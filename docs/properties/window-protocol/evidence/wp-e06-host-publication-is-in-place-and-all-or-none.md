# wp-e06-host-publication-is-in-place-and-all-or-none

## Discovery trigger

Record WP-E06 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Catalog revision 2 records the all-or-none contract as HEAD
behavior and marks it invalidated if shrink-first is adopted. #829's plugin
slice (#877) adopts it.

Exercised status: yes - the baseline preflight witnesses ("reports a
destination slot that stopped accepting writes and reads no candidate getter",
"replaces every slot and the length of an accepted destination in place",
"rejects containers whose element or length assignment could throw") ran in
the #875 `bun run check:repo` gate at `328ab11c`; #877 deleted the all-slot
preflight and replaced the failure contract with WP-P09.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Baseline run: #875 (head `328ab11c` locally; `2d58cc20` at read) records
  `bun install --frozen-lockfile && bun run check:repo ok`.
- #877 deletes `hostArrayReplacementRejection` and `replaceHostArrayContents`
  and adds `publicationRejection(target, S)` (`transform-capture.ts:807`) and
  `publishInPlace` (`:840`).
- `publicationRejection` checks extensibility, a writable `length`, and
  descriptors of output slots `[0, S)`; captured slots are rechecked at capture.
  The test at `transform-capture.test.ts:1193` asserts `not_array`, `proxy`,
  `not_extensible`, `length_not_writable`, and `slot_not_configurable`.
- The transform-edit-responses catalog marks TE20 `Status: invalidated` with a
  #829 note naming the replacement witnesses.
- In-place identity survives: "shrinks first and leaves exactly the candidate in
  the original array object" (`transform-capture.test.ts:1067`) asserts
  `target[0]` is the kept object.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #877 (#829 PR A; head `3844a179`): `bun run check:repo` ok (one unrelated
  flaky timer test passed 3 of 3 on rerun), fmt, clippy, `-p daemon`, fixture
  build ok; fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17
  skip, 0 fail.

## Failure scenario

A host array with a non-configurable slot inside the covered prefix. Under the
old contract publication refused before any write; under shrink-first the
shrink stops at that slot and WP-P09's restoration applies.

## Timing windows and dependencies

The synchronous publication section only.

## What a test must construct

Not applicable after #877 for the failure half. The preserved half: publish
into a plain array and assert identity and exact contents; publish into each
rejecting container shape and assert refusal before any write.

## Investigation log

### Q: Which clauses survive invalidation?

- Sources examined: Comment 3 disposition of TE20; #877 description.
- Findings: In-place identity and prepublication validation of the container
  and output slots survive; all-or-none failure does not.
- Missing evidence: None.
- Conclusion: resolved with answer: identity and validation preserved,
  all-or-none invalidated.

### Q: Did any recorded run exercise the baseline?

- Sources examined: #875 gate block; test names at `328ab11c`.
- Findings: `bun run check:repo` ran the plugin suite with the three baseline
  tests present.
- Missing evidence: None.
- Conclusion: resolved with answer: #875's run.
