# wp-p23-the-scan-traversed-a-hostile-slot

## Discovery trigger

Record WP-P23 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Ticket #832 pairs WP-P13 with this marker.

Exercised status: yes - "crosses planted proxies, accessors, and revoked
proxies without invoking any hook" places every hostile hop between the end
and the one readable match and asserts the scan returns that match with zero
traps; it ran in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun
run check:repo` gates. The array is a bare fixture, not a pass (see WP-P13).

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Marker: `expect(scanMessageIds(host, (id) => id === "target")).toBe(0)` with
  six hostile objects at indices 1 to 6 and an accessor at index 7.
- Filter traversal crosses the same slots: `charged` equals `host.length * 4`.
- Hostile entries (`rust-mode-window.test.ts:142-150`): a message proxy, an
  `info` proxy, an accessor `id`, an accessor `info`, a revoked message proxy,
  and a revoked `info` proxy at indices 1 to 6; index 7 holds a string that
  `Object.defineProperty` replaces with an accessor.
- The proxy handler counts `get`, `has`, `ownKeys`,
  `getOwnPropertyDescriptor`, and `getPrototypeOf`; every trap throws after
  counting.
- The in-test comment names the falsifier: "reads `host[i].info.id` before
  checking for a proxy and trips a trap here".
- Owner: #832 PR one, #883 (ticket #832: "trap counters at array slot,
  message, `info`, and `id` stay zero while the scan crosses planted proxies,
  accessors, and revoked proxies").

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

Not applicable to a marker; see WP-P13.

## Timing windows and dependencies

None.

## What a test must construct

Place the match below every hostile hop and assert the returned index.

## Investigation log

### Q: Are the hostile slots in a covered region of a real pass?

- Sources examined: `rust-mode-window.test.ts:123-164`.
- Findings: No pass runs; the array is a bare fixture.
- Missing evidence: A pass-level variant.
- Conclusion: unresolved, needs a pass-level witness or owner acceptance
  (needs human input).

### Q: Does an absent id make the scan cross every slot?

- Sources examined: `rust-mode-window.test.ts:153`.
- Findings: Yes: `scanMessageIds(host, (id) => id === "absent")` returns -1
  after crossing all slots with `traps` still 0.
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
