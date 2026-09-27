# wp-e12-revert-epoch-invalidates-retained-output

## Discovery trigger

Record WP-E12 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface store. Catalog revision 2 records epoch invalidation as existing
behavior. #830 replaces the native attachment cache with a revision-bound
store keyed by epoch; ticket #830 names WP-E12 as preserved.

Exercised status: yes - the #879 native-store and store-cut witnesses, #881's
interrupted-revert tests, and #833's epoch-bound previous-output tests
(`serialized_output_cache_take_under_a_new_epoch_returns_nothing`,
`a_same_pass_reset_reuses_no_previous_output`) ran in their PRs' daemon
gates.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #879 (head `7a3db43b`) `cargo test -p daemon ok`; #880
  (head `748c0c4b`) repeats it after deleting the old cache.
- `NativeOutputStore` (`lib.rs:2368`): one entry per session `{ revert_epoch,
  retained_bytes, output }`; `remove` always takes the entry out and
  `into_previous` (`:2317`) returns it only when epoch and revision match; a
  `revert_epoch` change drops the entry.
- `truncate_history_segments_for_revert`
  (`crates/memory-store/src/lib.rs:12001`) separates a real cut from a no-op;
  its test asserts epoch 1 and `row_version + 1` for the cut and equal values
  for the repeat.
- #833 deletes the per-message serialized-output memo, its lookup, and
  `serialized_output_cache_revert_epoch_bump_evicts_session` (`6477c9f2`).
  `SerializedOutputCache` (`crates/daemon/src/transform.rs:397`, field
  `serialized_outputs` at `lib.rs:2509`) now holds one D13 previous output
  per session; `take_previous_output` (`transform.rs:435`) removes the entry
  and returns it only at the recorded `revert_epoch`. Witnesses:
  `serialized_output_cache_take_under_a_new_epoch_returns_nothing` (`:27963`),
  `serialized_output_cache_evicts_the_least_recently_recorded_session`
  (`:27976`), and `a_same_pass_reset_reuses_no_previous_output`
  (`revision_3.rs:701`: a repeat with nonempty previous keeps, then a D10
  reset whose response has none, for CK and native).
- #881 (head `1c66f16c`): `assert_folded` (`revision_3.rs:1120` at `f2442b2f`)
  asserts `revert_epoch + 1` once across a CAS conflict (`:1147`) and a panic
  plus reopen (`:1163`) after the truncate commit, and `assert_steady` (`:1132`)
  re-enters the truncate and asserts neither epoch nor row version moves; both
  ran in the #881 `cargo test -p daemon` gate (head `1c66f16c`). #881 lists
  WP-E12 under "existing checks green".

Gate results of the runs cited here, as recorded in the PR descriptions:

- #879 (#830 PR 1; head `7a3db43b`): fmt, clippy, `-p daemon`, doctests,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  23 pass, 17 skip, 0 fail (steady-state byte identity included).
- #880 (#830 PR 2; head `748c0c4b`): fmt, clippy, `-p daemon`, doctests,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  23 pass, 17 skip, 0 fail.
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

A revert cuts history; the retained output from before the cut is reused as a
previous base, so reverted content reappears in the prompt.

## Timing windows and dependencies

The retained entry outlives the cut until the next lookup.

## What a test must construct

Store an output at epoch e, cut history (epoch e + 1), and assert the next
lookup returns nothing and the recipe uses literals. Repeat the cut with the
refreshed row version and assert neither epoch nor row version changes.

## Investigation log

### Q: Does the native output store drop entries on an epoch change?

- Sources examined: #879 description;
  `native_output_store_enforces_entry_cap_lru_and_revert_epoch`.
- Findings: Yes; the test covers entry cap, total budget, LRU order, revision
  and epoch rules, and no double charge.
- Missing evidence: None.
- Conclusion: resolved with answer: yes.

### Q: Does the CAS conflict after a truncate bump the epoch twice?

- Sources examined: #881 interrupted-revert tests; store no-op test.
- Findings: The store test pins the no-op; #881's interrupted-revert tests pin
  one bump end to end.
- Missing evidence: None.
- Conclusion: resolved with answer: one bump, at the store level and end to
  end in #881's daemon gate.
