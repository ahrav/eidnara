# wp-p12-boundary-index-is-fixed-through-publication

## Discovery trigger

Record WP-P12 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Revision 3 captures a suffix at a fixed index; a host edit can
shift the suffix while the pass awaits.

Exercised status: yes - "declines ${name} during the await with no candidate
write and no promotion" constructs prefix deletion, same-length reorder, root
rebinding, and interior window omission during the await; "declines when the
host moves the discovered anchor before the window is copied" and "declines
cleanly when the host replaces its root array before a rerun" cover the
discovery and rerun gaps. All ran in the #883 (head `d7712d75`) and #884 (head
`f8734c12`) `bun run check:repo` gates.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded runs: #883 (`window-protocol/m1-plugin-revision-3`, head
  `d7712d75`) and #884 (head `f8734c12`) `bun run check:repo ok`. At
  `f2442b2f` the rechecks are `recheckCapture("wire-build")` (`:1406`),
  `"series-restart"` (`:1562`), and `"publish"` (`:1641`).
- The family holds the second pass's response, asserts the enabling state
  (`bodies[1].boundary` equals `m-6` and `native_messages` has length 4),
  applies the mutation, resolves with an insert candidate, and asserts
  `output.messages` equals the mutated array, contains no candidate,
  `boundary` stays `m-6`, and `chargedBytes` is 0.
- The transform-edit-responses catalog TE17 note at `f2442b2f` records the
  preservation and scoping to the window.
- #883 Deviations: "root properties outside the window are unchecked (walking
  them is O(N))".

Gate results of the runs cited here, as recorded in the PR descriptions:

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

The host deletes a covered message while the response is pending. The window
now starts one slot earlier; a rescan finds the anchor at the new index and
applies the old recipe there, overwriting the wrong slots.

## Timing windows and dependencies

Between send and response; also between discovery and copy (WP-P01).

## What a test must construct

Use the deferred fake transport; mutate after the send with each shape; assert
the published array equals the mutated host, no candidate slot, and no
promotion.

## Investigation log

### Q: Is each mutation shape a separate run?

- Sources examined: `rust-mode-window.test.ts:284-338`.
- Findings: Four named mutations run as separate tests.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; they ran in the #883 and #884 `bun
  run check:repo` gates.

### Q: Are root properties outside the window part of the capture?

- Sources examined: #883 Deviations.
- Findings: No; #883 records it as an implementer deviation. The description
  records no owner decision on it.
- Missing evidence: Owner acceptance of the deviation.
- Conclusion: unresolved, needs owner acceptance of #883's recorded deviation
  (needs human input).

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
