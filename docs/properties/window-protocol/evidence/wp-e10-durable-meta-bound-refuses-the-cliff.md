# wp-e10-durable-meta-bound-refuses-the-cliff

## Discovery trigger

Record WP-E10 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface store. Catalog revision 2 records the 512 KiB guard and a
fixture-specific cliff. #826 must leave the pin untouched; #833 replaces the
pin with a window assertion.

Exercised status: partial - the guard is unchanged. #833's
`a_hundred_thousand_message_session_commits_a_three_hundred_message_window`
replaces the cliff pin and ran in #833's `cargo test -p daemon` gate; #833's
acceptance run measures meta across N. No witness constructs a refused
commit.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #873 lists "the meta cliff pin" among green existing
  goldens; `cargo test -p daemon ok`.
- At `265df096` (catalog revision 2's HEAD) the test iterated `[(1_000, true),
  (1_400, false)]` (`transform_meta_bound.rs:20-21`). At `43e88bdc` (M1 base)
  and `f8734c12` it iterated `[1_400, 10_000]` and asserted each commits under
  128 KiB (`:130-139` at `f8734c12`). `e15a09a6` made the change.
- At `f8734c12` the helper `first_hard_pass` asserted
  `block_identity_by_mid.len() == count` (`:183-186`). Meta stays flat
  because `e15a09a6` moved the map out of the blob: the field is
  `#[serde(skip)]` (`crates/memory-store/src/lib.rs:2110-2111`) and persisted
  in the `block_identities` table.
- #881 (head `1c66f16c`) changed the test request to `v: 3`, `boundary: null`
  and dropped the covered-drift half of the flat-meta test (renamed from
  `meta_bytes_stay_flat_as_covered_history_grows_and_covered_drift_still_rejects`,
  `:139` at `748c0c4b`).
- #833 (`7a8fb84b`) deletes both tests and adds
  `a_hundred_thousand_message_session_commits_a_three_hundred_message_window`
  (`crates/daemon/src/transform_meta_bound.rs:93`): `SyntheticHistory::mixed`
  seeds 50,000 segments, a 300-message window commits a HARD, coverage is at
  100,000, the identity rows equal the window, and meta is under 128 KiB.
  #833 also deletes covered-drift rejection from production (D25).
- #873 and #874 measure `cache_state.meta` at 145,784, 147,023, and 142,732
  bytes at N = 10k, 100k, 1M under revision 2 requests (whole arrays). #833's
  acceptance run (commit `cf89a9c2`, Rust 1.98.1, release test build, W =
  300) measures 117,066, 117,040, and 117,064 B: equal entry counts, the 26 B
  spread only decimal digits. `tail_hygiene_baseline` is 74,502 B of that
  (`694c7545` keeps per-block entries in the form the previous build reads).

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

A session's meta crosses 512 KiB; every later pass fails its commit, so the
session never folds again.

## Timing windows and dependencies

None; growth accumulates across passes.

## What a test must construct

Commit a first HARD over a large covered session and assert meta bytes and row
presence; feed an input one byte over the bound and assert refusal with no row
and no meta change; at fixed window, compare meta bytes across N.

## Investigation log

### Q: Does a current witness cover refusal without effect?

- Sources examined: `transform_meta_bound.rs` at `f2442b2f`; `git log` of the
  file.
- Findings: No. Both current tests assert success. Store-level tests exercise
  `MAX_DURABLE_TEXT_BYTES` for other fields (for example
  `crates/memory-store/src/lib.rs:28188`), not a refused transform commit.
- Missing evidence: A refused-commit witness.
- Conclusion: unresolved, needs a refused transform commit test; queued in
  `portfolio-evaluation.md`.

### Q: Does meta stay below 128 KiB with a 300-message window at N = 100k?

- Sources examined: `transform_meta_bound.rs:93`; #833 acceptance run.
- Findings: Yes. The regression commits at 100,000 messages under 128 KiB;
  the acceptance run reads 117,066 B at N = 10k to 117,064 B at N = 1M.
- Missing evidence: The owner's reading of "equal across N" given the
  decimal-digit spread (needs human input).
- Conclusion: resolved with answer for the 128 KiB bound.
