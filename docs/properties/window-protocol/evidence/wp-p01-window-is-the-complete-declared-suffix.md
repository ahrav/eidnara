# wp-p01-window-is-the-complete-declared-suffix

## Discovery trigger

Record WP-P01 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Revision 3 replaces whole-array requests with a declared
suffix. Tickets #831 (daemon side) and #832 (plugin side, with WP-P17) carry
the obligation.

Exercised status: yes - #875's `impossible_declarations_are_invalid_params`
refuses a window head that is not the declared mid at the module seam; #881's
`boundary_presence_head_sequence_and_duplicates_are_invalid_params` refuses a
head that is not the mid, a duplicate mid, and sequences outside +/-(2^53 - 1)
at the handler with no state change, and
`paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request`
checks them on an assembled paged body (both in the #881 `cargo test -p daemon`
gate (head `1c66f16c`)); #883's "sends the declared window in both
representations and publishes the recipe at boundaryIndex + i", "rejects a
duplicate id inside the window and ignores one outside it", the
interior-omission case (WP-P17), and "declines when the host moves the
discovered anchor before the window is copied" construct the plugin side and ran
in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run check:repo`
gates.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- #875 (recorded run: `cargo test -p daemon ok (2,766)`):
  `impossible_declarations_are_invalid_params` asserts errors containing "not
  at the declared boundary" and "does not start at" for a window head that is
  not the declared mid.
- #881 (head `1c66f16c`, `cargo test -p daemon ok`):
  `boundary_presence_head_sequence_and_duplicates_are_invalid_params` covers a
  missing boundary, head not equal to the mid, sequences beyond +/-(2^53 - 1),
  a duplicate mid, and a declared row newer than rendered, each
  `invalid_params`, with `row_version` and meta unchanged; the largest safe
  sequence is admitted and answers `boundary_unknown`.
- #883 (head `d7712d75`, `bun run check:repo ok`; also in #884's gate): the
  steady-state window test asserts `body.native_messages` equals
  `first.slice(6)`, CK mids `m-6` to `m-9`, and no `ordinal` in the body; the
  duplicate test asserts a decline `unsupported_source (duplicate id m-5)` and
  no second body, while a duplicate outside the window is ignored.
- `copyWindow` copies through indexed own descriptors; `scanMessageIds`
  (`transform-capture.ts:101`) fixes `boundaryIndex` in the same synchronous
  section (#883: "`boundaryIndex` is fixed in the capturing synchronous
  section").
- Review finding P4 on #832 (round 1): on the discovery path `boundaryIndex`
  is computed before an await, so the host can change between the scan and the
  copy. #883 lists the fix as accepted ("window head recheck after
  discovery"): a head that is no longer the declared mid declines
  `source_changed`, and "declines when the host moves the discovered anchor
  before the window is copied" (`rust-mode-window.test.ts:845`) constructs the
  move.
- #831 is #881 (head `1c66f16c`); its daemon tests ran in the `cargo test -p
  daemon` gate its description records. #832 is #883 (head `d7712d75`) and
  #884 (head `f8734c12`); their plugin tests ran in the `bun run check:repo`
  gate each description records.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `-p
  daemon` ok (rerun after a known timing flake), `-p memory-store`, storage
  no-default check, `-p host-runtime --test protocol_vectors`, doctests,
  markers, `check:repo`, fixture build ok; fixture-contract 6; manifest ok;
  `test:rust` 9 pass, 14 fail alone (by design), 23 pass, 17 skip with #883.
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

The host array is `[c1..c5, t1, t2, t3]` with boundary `t1`. A capture that
sends `[t1, t3]` passes a length check and loses `t2`.

## Timing windows and dependencies

Between the id scan that fixes `boundaryIndex` and the copy; any await in
between allows the host to shift the suffix.

## What a test must construct

A covered prefix with at least three tail messages; a candidate that omits an
interior message (WP-P17); duplicate ids inside and outside the window; a head
that is not the declared mid. Assert membership independently of the encoder.

## Investigation log

### Q: Can the discovery await shift the window before the copy?

- Sources examined: #832 review P4.
- Findings: It could before the P4 fix (the index was fixed before the final
  discovery page resolved). At `f2442b2f` the window head is rechecked after
  discovery and a moved anchor declines `source_changed`.
- Missing evidence: None.
- Conclusion: resolved with answer: the shift is detected and declined; the
  witness ran in the #883 and #884 `bun run check:repo` gates.

### Q: Does the daemon refuse duplicates inside the window?

- Sources examined: #881 description; `revision_3.rs:193`.
- Findings: `resolve_window` refuses duplicate mids with `invalid_params`.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; the test ran in #881's daemon gate.

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

## Update for #1060 at `07ea883b5`

Tickets #1062, #1063, and #1065; PRs #1069 (head `6d527babf`) and #1070
(head `dbdac1150`).

- Contract versus code at `08de9051a`: the record's "null submits the whole
  host array" disagreed with wire section 7.10 and with `coldHead`, which
  narrowed a null window above half the cap to its newest suffix. #1060's
  property catalog directs a correction.
- Wire: `docs/host-wire-protocol.md:983` (the `boundary` row), `:1010` (the
  window rules), and `:1024` (the cold-import rule) state that the half-cap
  suffix applies while the consumer's configuration lets Eidnara fold and
  that a consumer whose configuration withdraws fold authority submits its
  whole array.
- Code: `coldHead` clears a pinned head and returns `undefined` when the
  source has no `measure`, before any lookup
  (`packages/opencode-plugin/src/hooks/context/transform-session-client.ts:863-866`),
  so a pin left by a folding run does not narrow a later native-configured
  window (`packages/pi-plugin/src/transform/pi-transform.test.ts:505`;
  `transform-session-client.test.ts:894`). A response naming native
  authority clears the pin too and declines a cold-narrowed pass
  (`:1669-1680`); later null passes send the whole array while
  `state.nativeAuthority` holds (`:1361`).
  The reset is visible without an anchor: the first suffix starts at
  `host[1]` and the suffix after the authority settles at `grown[3]`
  (`rust-mode-window.test.ts:1389-1414`), while the wire text keeps the same
  pinned message until the first anchor exists; recorded as a disagreement.
  Tool pairing moves the head: `windowStart` (`:898-903`) walks later past a
  tool result whose call is outside the suffix, or back to the message
  carrying the calls, which can carry the suffix past half the cap, as the
  wire permits (`docs/host-wire-protocol.md:1024`) and
  `packages/pi-plugin/src/transform/pi-transform.test.ts:488` constructs.
  Pi spreads `measure` only under `inputs.eidnaraFolds`
  (`packages/pi-plugin/src/transform/pi-transform.ts:268-275`), set per run
  from the session's project configuration
  (`packages/pi-plugin/src/index.ts:571`, `:586`). OpenCode spreads it only when
  `compactionOff` is not `true`
  (`packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts:397`).
- Lead slots: Pi sets `coldLead: 1` when the first message is a compaction
  summary (`pi-transform.ts:280`), and `coldSource` serves `host[:lead]`
  followed by `host[head:]` (`transform-session-client.ts:906-913`). The
  folding cold window is therefore the lead slots plus the pinned suffix,
  not a contiguous suffix; the wire text at `:955` and `:982` names only
  the suffix.
- Witnesses: `packages/pi-plugin/src/transform/pi-delivery.test.ts:341` (401
  rows whole under native authority; the 400-block suffix and its pin while
  folding) and `:390` (the whole array after a cold suffix is answered under
  native authority);
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1417`
  (whole array and publication by identity), `:1444` (suffix and pin),
  `:1342` (decline, then the whole array), and `:1389` (the pinned suffix
  returns once a response reports another authority).

### Q: Does the null-window clause hold under native authority?

- Sources examined: the files above.
- Findings: Yes. Both consumers submit the whole host array with a null
  boundary when the configuration withdraws fold authority, whether or not
  an earlier folding pass pinned a head; a cold suffix answered under native
  authority is declined and the next null passes send the whole array; the
  folding arms keep the half-cap suffix and its pin.
- Missing evidence: None at `07ea883b5`.
- Conclusion: resolved with answer; the record carries the authority
  qualifier.

### Q: Is the folding null window a contiguous suffix?

- Sources examined: `pi-transform.ts:280`;
  `transform-session-client.ts:860-913`; `docs/host-wire-protocol.md:983`,
  `:1010`.
- Findings: No when lead slots exist. A leading Pi compaction summary is one
  lead slot, served ahead of the pinned suffix. The wire text describes only
  the suffix.
- Missing evidence: A wire statement of the lead slots.
- Conclusion: unresolved, needs the wire document owner to state the lead
  slots in the wire and every peer in one change, or the consumers to drop
  them; the record's guarantee states the wire rule and reports the code's
  lead-slot membership as the disagreement (needs human input).
