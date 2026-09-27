# wp-p24-wire-admissibility-is-explicit

## Discovery trigger

Record WP-P24 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface protocol. Revision 3 changes the request meaning. Ticket #831 carries
the daemon admissibility; #832 the plugin side.

Exercised status: yes - #875's discovery shape (unsafe `before_sequence`
including `i64::MIN` and `i64::MAX`, malformed bodies `invalid_params`, unknown
methods still `unrecognized_request_shape`) ran in the #875 daemon gate. #881's
`a_missing_or_non_3_revision_is_refused_with_expected_and_received_and_no_state_change`,
`boundary_presence_head_sequence_and_duplicates_are_invalid_params`,
`a_boundary_that_does_not_decode_is_bad_request`, and
`paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request`
ran in the #881 `cargo test -p daemon` gate (head `1c66f16c`), and #881's
`test:rust` against the revision 2 plugin is red by design (9 pass, 14 fail).
The plugin side ("serves raw and logs an upgrade hint when the daemon refuses
the revision", the "a revision 2 daemon" decline, and the final-page test in
`module-wire.test.ts`) ran in the #883 (head `d7712d75`) and #884 (head
`f8734c12`) `bun run check:repo` gates; the paired `test:rust` is 23 pass, 17
skip, 0 fail in #881 (with #883), #883, and #884.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- #875 recorded run: `before_sequence` and `sequence` must be JavaScript safe
  integers; `i64::MIN` passed the check before review and is now refused.
- #881 (head `1c66f16c`): `TransformRequest.v` holds the raw JSON value;
  `boundary` is `Option<Option<BoundaryAnchor>>`; the revision check runs
  first (`lib.rs:8224-8232`); the refusal test covers `v` 2 with ordinals,
  missing, `"3"`, `3.0`, and `4` with messages `expected transform revision 3,
  received <v>` and an unchanged `durable` state.
- #881 paging test (`revision_3.rs:287`): `v` 2 and a missing boundary on the
  assembled request are refused; `v` on a non-final page answers
  `authority_transform_page_protocol_mismatch`; the assembled valid request is
  served with `boundary` `m4`.
- #831 review F7 (decode failures are `bad_request`): #881 lists it as fixed
  with a table test, `a_boundary_that_does_not_decode_is_bad_request`
  (`revision_3.rs:231`), which covers a missing member, `5`, `1.5`, `"2"`, 2^63,
  and `v: 2` with a malformed boundary ("decoding precedes the revision check");
  `docs/host-wire-protocol.md:972` and `:981` at `1c66f16c` document it.
- #883 (head `d7712d75`): `transform_revision_unsupported` serves raw and logs
  `daemon_revision_unsupported` with an upgrade hint; a revision 2 daemon's
  discovery answer declines with `discovery_declined` and an upgrade warning;
  review P9's paging test is `module-wire.test.ts:901` (`v` and `boundary` on
  the final page only). All ran in the #883 (head `d7712d75`) and #884 (head
  `f8734c12`) `bun run check:repo` gates.
- Late commit `2d58cc20` on #875 bounds anchor pages below -(2^53 - 1); its
  witness `an_unsafe_negative_sequence_is_not_listed`
  (`crates/daemon/src/window_coverage/tests.rs:655`) ran in the #881 and #833
  daemon gates.

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
- #883 (#832 PR one; head `d7712d75`): `bun install --frozen-lockfile`,
  `check:repo`, fmt, clippy, markers, fixture build ok; fixture-contract 6;
  manifest ok; `test:rust` 23 pass, 17 skip, 0 fail.
- #884 (#832 PR two; head `f8734c12`): `bun install --frozen-lockfile`,
  `check:repo`, fmt, clippy, markers, fixture build ok; fixture-contract 6;
  manifest ok; `test:rust` 23 pass, 17 skip, 0 fail.
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `3ebfc3b9`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 108, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

A revision 2 plugin sends a whole array with ordinals and no `v`. A daemon
defaulting `v` to 2 would accept it and apply revision 3 coverage to it.

## Timing windows and dependencies

Process restarts in either order during an upgrade.

## What a test must construct

Send revision 2 bodies, missing and non-3 `v`, missing `boundary`, off-head
windows, unsafe sequences, and paged variants to the revision 3 daemon; answer
discovery with `unrecognized_request_shape` and transforms with
`transform_revision_unsupported` to the plugin; assert codes, messages, and no
durable change.

## Investigation log

### Q: Is skew exercised end to end?

- Sources examined: #881, #883, and #884 gate blocks.
- Findings: #881 alone: `test:rust` 9 pass, 14 fail, by design against the
  revision 2 plugin. Paired with #883: 23 pass, 17 skip, 0 fail, recorded in
  #881, #883, and #884.
- Missing evidence: None.
- Conclusion: resolved with answer: both the refusal direction and the paired
  pass are recorded.

### Q: Are typed-decoding failures distinguishable?

- Sources examined: #831 review F7; `revision_3.rs:231`;
  `docs/host-wire-protocol.md` at `1c66f16c`.
- Findings: They answer `bad_request`, whatever `v` is; the wire document's
  error table (`:1336`) and revision note (`:1345`) say so.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; the table test ran in #881's daemon
  gate.
