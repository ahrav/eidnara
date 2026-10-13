# wp-p22-the-host-mutated-during-the-await

## Discovery trigger

Record WP-P22 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Ticket #832 pairs WP-P12 with this marker.

Exercised status: yes - the fixed-window family "declines ${name} during the
await with no candidate write and no promotion" mutates the host by prefix
deletion, same-length reorder, root rebinding, and interior window omission
between send and response, with the enabling state asserted first; it ran in
the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run check:repo`
gates.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Enabling state: `await Promise.race([reached.promise, pass])`, then
  `bodies[1].boundary` is `m-6` and `native_messages` has length 4.
- Shapes: `splice(0, 1)`; swap of slots 1 and 7; `output.messages =
  [...output.messages]`; `splice(8, 1)` plus push.
- Each case first runs a folding pass so the second pass declares `m-6` from
  state, then holds that pass's response with `pending`; the fake resolves
  `reached` when the second transform body arrives.
- The released response carries `boundary` `m-9` and an insert candidate; the
  assertions show neither the candidate nor the new boundary is promoted.
- `defaultTransformCaptureAdmission.chargedBytes` is 0 after each case, so the
  capture lease was released.
- Owner: #832 PR one, #883 (ticket #832: "prefix deletion, same-length
  reorder, and root rebinding during the await through the deferred fake
  transport, each declined").

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

Not applicable to a marker; see WP-P12.

## Timing windows and dependencies

Between send and response.

## What a test must construct

Hold the response, assert the enabling state, mutate, release.

## Investigation log

### Q: Is the mutation between discovery and copy constructed?

- Sources examined: #832 review P4; `rust-mode-window.test.ts:845`.
- Findings: Yes at `d7712d75`: "declines when the host moves the discovered
  anchor before the window is copied" moves the anchor between discovery and
  the copy and asserts the decline.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; it ran in the #883 and #884 `bun run
  check:repo` gates.

### Q: Are the shapes run in separate tests?

- Sources examined: `rust-mode-window.test.ts:301-338`.
- Findings: Yes; one `it` per entry of the `mutations` record, four in total.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; it ran in the #883 and #884 `bun run
  check:repo` gates.

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

## Re-evidence 2026-10-13 (#996)

The await-mutation witnesses still run with the mutation after send and before
the response at a nonzero boundary, now over a capture that kept its verified
tapes. "rejects a hook planted in a verified prefix message while the pass
awaits the daemon, without running it" adds a pass whose deferred response
receives the planted accessor and declines at publish.
