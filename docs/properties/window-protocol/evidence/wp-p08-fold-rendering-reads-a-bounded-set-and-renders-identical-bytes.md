# wp-p08-fold-rendering-reads-a-bounded-set-and-renders-identical-bytes

## Discovery trigger

Record WP-P08 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. D14 bounds the fold read. Ticket #826 carries WP-P08 with both
falsifiers.

Exercised status: yes -
`bounded_fold_matches_the_full_read_over_the_store_shape_fixture` (388 rows,
four budgets),
`bounded_fold_matches_the_full_read_over_sixty_thousand_segments` (13 budgets
from 20 to 10,000,000), `bounded_fold_work_is_independent_of_history_length`,
and `bounded_m1_matches_the_full_read_and_withholds_an_overflowing_body` ran
in the #873 gates; both named falsifiers differ from the reference within the
sweep.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded run: #873 gates (`daemon ok`, `memory-store ok`, `storage ok
  (107)`); goldens green.
- Evidence (#873): m0 bounded versus full byte-identical over the 388-row
  store-shape fixture at four budgets and a 60,000-segment session
  (importances 1 and 100 among the newest 249, legacy rows older than K) at 13
  budgets; m1 identical for 0, 1, 200, and 259 new rows; overflow withheld at
  260.
- Rows visited per fold at most 249 + K + L (+2 edge lookups); m0 and m1
  (rows, VM_STEP) equal at H = 4,000 and H = 60,000.
- One-time legacy capture at H = 60,000: 5 rows, 300,020 VM steps.
  `ModuleMeta::legacy_history_segment_seqs`
  (`crates/memory-store/src/lib.rs:2052`) persists the list; every writer that
  can add a legacy row clears it.
- Behavior note (#873): when more rows than the m1 cap sit above the folded
  sequence, the pass folds instead of serving SOFT with a huge m1.
- #874 inventory: m0 segments 2489 rows / 59,848 VM steps at H = 50,000,
  within 249 + K + L.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #873 (#826 PR 1; description gate head `d5bb4efb`, head at read `efb3fe9c`):
  fmt, clippy, `test -p storage` (107), `-p memory-store`, `-p daemon`,
  doctests, `check -p storage --no-default-features`, comment markers, fixture
  build all ok; `test:fixture-contract` 6 pass; `validate-mode-manifest` ok;
  `test:rust` 23 pass, 17 skip, 0 fail.
- #874 (#826 PR 2; description head `704568ec`, head at read `c239461c`): fmt,
  clippy, `-p memory-store`, `-p daemon`, doctests, storage no-default check,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  0 fail, 17 skip.
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `test -p
  daemon` ok (one run hit a known timing flake in a 33-test binary; the rerun
  was green), `-p memory-store`, `check -p storage --no-default-features`, `-p
  host-runtime --test protocol_vectors`, doctests, markers, `bun run
  check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin
  is refused) and 23 pass, 17 skip, 0 fail with #883.
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `3ebfc3b9`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 108, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

An implementation reads the newest K rows and computes pressure from them; an
importance among positions K+1 to 249 changes p, so the tiers and the rendered
m0 differ from the full read.

## Timing windows and dependencies

Retry multiplier raises p and lowers K across attempts; the read set must
cover every attempt.

## What a test must construct

Sweep budgets over a large synthetic history with mixed importances and legacy
rows; render by the bounded and full reads; compare bytes and coverage fields;
run each falsifier and assert it differs.

## Investigation log

### Q: Do the falsifiers fail the differential?

- Sources examined: #873 description.
- Findings: "Both named falsifiers ... differ from the reference within the
  sweep."
- Missing evidence: None.
- Conclusion: resolved with answer: yes.

### Q: Is a legacy list trusted after an unordered set?

- Sources examined: Commit `efb3fe9c` message.
- Findings: That commit returns `HistorySegmentRangesOutOfOrder` from the
  capture scan and persists no list for a failing set.
  Its witnesses, `fold_capture_refuses_stored_ranges_out_of_order`
  (`crates/memory-store/src/lib.rs:22796`) and
  `a_fold_over_stored_ranges_out_of_order_fails_closed`
  (`crates/daemon/src/transform.rs:20060`), ran in the #881 and #833
  `-p memory-store` and `-p daemon` gates.
- Missing evidence: A recorded measurement of the scan's cost (the commit
  message's 480,010 VM steps at H = 60,000 is in no description).
- Conclusion: resolved with answer: no list is trusted after an unordered
  set.
