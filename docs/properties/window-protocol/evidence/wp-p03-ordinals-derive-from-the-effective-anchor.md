# wp-p03-ordinals-derive-from-the-effective-anchor

## Discovery trigger

Record WP-P03 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Revision 3 deletes the plugin ordinal memo; the daemon becomes
the only ordinal authority. Ticket #827 carries the independent-model
comparison.

Exercised status: yes - `resolved_ordinals_equal_the_independent_model`
(multi-block anchor, interior covered removal, lineage continuation base 40,
synthetic head, middle, and tail) and
`synthetic_borrowing_matches_the_plugin_rule_case_for_case` (every pattern up
to eight messages) ran in the #875 daemon gate.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #875 daemon gate.
- The model test comments name the cases: "Row 3 ends at block 2 of message
  m5: a multi-block anchor", "A stale declared row with and without the
  covered interior message m3", "No anchor under a lineage continuation base:
  synthetic head, middle, and tail" with continuation base 40.
- The synthetic test enumerates every synthetic pattern up to eight messages
  and compares with the plugin's former rule (#875 description: "mirrors
  `annotateOrdinals` for fully resolved windows; the one place the plugin
  differs, dense numbering of an unpersisted suffix, is pinned in the test
  doc").
- #883 deletes `ModuleOrdinalMemo`, `primeOrdinalMemo`, `annotateOrdinals`,
  `invalidateOrdinals`, and the continuation base in the plugin; its
  description records 0 non-test hits.
- #881 (head `1c66f16c`): an ingress `ordinal` is overwritten and
  `ck.meta.ordinal` cleared; `absolute_ordinal` and `declared_trim` are gone.
  The no-`ordinal` assertion in
  `each_resolution_outcome_runs_through_the_handler_with_its_effects` ran in
  the #881 `cargo test -p daemon` gate (head `1c66f16c`).

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
- #883 (#832 PR one; head `d7712d75`, base
  `window-protocol/m1-daemon-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).

## Failure scenario

The host deletes covered message 3. A position-based rule renumbers the window
one lower, so coverage believes the anchor moved and tool-arc splitting uses
wrong ordinals.

## Timing windows and dependencies

None within a pass.

## What a test must construct

Resolve windows over a multi-block anchor with and without a deleted covered
interior message, with synthetic messages at head, middle, and tail, and with
a continuation base; compare with an independent model.

## Investigation log

### Q: Is the plugin's dense unpersisted-suffix rule reproduced?

- Sources examined: #875 description; test doc of the synthetic test.
- Findings: No; the difference is pinned and documented. Revision 3 windows
  are resolved by the daemon, so the plugin rule no longer runs.
- Missing evidence: None.
- Conclusion: resolved with answer: documented difference, not a defect.

### Q: Does any ordinal reach served output?

- Sources examined: #881 description; `revision_3.rs:323`.
- Findings: The handler test asserts served `messages` and `operations`
  contain no `"ordinal"` and the response has no `coverage_ordinal` or
  `ordinal_continuation_base`.
- Missing evidence: None.
- Conclusion: resolved with answer: no; the test ran in #881's daemon gate.
