# wp-p20-the-shrink-actually-threw

## Discovery trigger

Record WP-P20 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Ticket #829 pairs WP-P09 with this marker.

Exercised status: yes - "restores the captured references after a shrink
stopped by a planted non-configurable slot" plants a non-configurable slot at
k with S <= k on a real Bun array and asserts the `TypeError`, length k + 1
before restoration, and no candidate entry; it ran in the #877 `bun run
check:repo` gate.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Assertions: `candidate.length <= k`; `publicationRejection(target,
  candidate.length)` null; `failure.error` a `TypeError`;
  `failure.shrunkLength` k + 1; `lengths` `[k + 1]`; detail names the stop
  length and the restored count.
- Recorded run: #877 (head `3844a179`).

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

Not applicable to a marker; see WP-P09.

## Timing windows and dependencies

None.

## What a test must construct

Plant the slot, assert `publicationRejection` passes, publish, and observe the
throw.

## Investigation log

### Q: Does the Bun version matter?

- Sources examined: #873 artifact identity (bun 1.3.14); catalog revision 2
  ("the ordering is verified in this Bun").
- Findings: The recorded runs use Bun 1.3.14.
- Missing evidence: A run on any other Bun version.
- Conclusion: resolved with answer for Bun 1.3.14.

## Dated note (2026-10-12, #989)

Dated note, 2026-10-12, #989 (spec #988 amendment A1): the host wire has one
unversioned contract, so every clause here that names a protocol version,
numbered revision, version-skew refusal, or retired-field refusal is
invalidated. At the #989 head, routed envelope, context, memory-capture,
note-evaluation, discovery, and transform bodies carry no `v`; transform
ignores a field named `v` and the former carriers, strict bodies refuse them
as unknown fields, `transform_revision_unsupported` and
`transform_tail_delta_retired` no longer exist, `transform/revision_3.rs` is
`transform/window_admission.rs`, `transform/revision_goldens.rs` is
`transform/recorded_goldens.rs`, and the golden fixture `transform-revision-2-goldens.json` is now
`transform-recorded-goldens.json`; `serve_native` remains until #993. Line
anchors into files #989 changed are as of their earlier recorded head.

## Dated note (2026-10-12, #993)

Dated note, 2026-10-12, #993 (spec #988 milestone U3): the transform request
carries no `serve_native`. The `pi` and `opencode-aisdk` profiles send
`native_messages` as their only window, the daemon derives the pass's CK from
it through the profile's codec before any projection and always serves native
recipes, and the `owned-llmrunner` and `claude-code-anthropic` profiles send
`messages`. Every clause here that names `serve_native`, a CK window sent
beside the native window, or the Pi check that aligned rows with `messages` is
invalidated; line anchors into files #993 changed are as of their earlier
recorded head.
