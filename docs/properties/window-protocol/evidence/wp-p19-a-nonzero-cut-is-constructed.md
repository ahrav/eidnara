# wp-p19-a-nonzero-cut-is-constructed

## Discovery trigger

Record WP-P19 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Ticket #827 pairs WP-P05 with this marker.

Exercised status: yes -
`a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input`
resolves `StaleSlice` at cut 4 with an input keep; it ran in the #875 daemon
gate. At the handler, #881's
`stale_slice_input_keeps_address_the_submitted_native_window` asserts
`StaleSlice { cut: 2 }` and a nonempty set of input keeps in the same pass; it
ran in the #881 `cargo test -p daemon` gate (head `1c66f16c`).

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Test doc: "at a stale cut of four, input keeps built against the submitted
  window start at the cut and reconstruct the served array from the unsliced
  input."
- #881 (head `1c66f16c`):
  `stale_slice_input_keeps_address_the_submitted_native_window` asserts the
  resolution is `StaleSlice { cut: 2 }` and `!keeps.is_empty()` before
  checking coordinates, so the marker fires on a correct implementation.

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

Not applicable to a marker; see WP-P05.

## Timing windows and dependencies

Between two plugin passes.

## What a test must construct

Publish a segment after the declared anchor, resolve, and assert the cut and
an input keep before reconstruction.

## Investigation log

### Q: Is the cut observed at the handler, not only the module?

- Sources examined: #881 description; `revision_3.rs:988`.
- Findings: Yes; the handler-level test asserts the cut and the input keeps.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; it ran in #881's daemon gate.

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
