# wp-e04-publish-requires-current-identities-and-epoch

## Discovery trigger

Record WP-E04 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface store. Catalog revision 2 records the publication fence as existing
behavior. #826 changes the set-generation fence and must keep the identity and
epoch semantics.

Exercised status: yes -
`selected_range_identity_drift_during_await_rejects_without_cooldown`,
`tail_identity_extension_during_await_still_publishes`, and
`publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict`
construct drift, a legal extension, and an epoch change; #873 adds
`publish_rejects_a_firing_whose_set_was_truncated_and_regrown_to_the_same_maximum`
for the new `MAX(sequence)` set fence. All ran in the #873 gates (`memory-store
ok`, `daemon ok`).

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #873 (description gate head `d5bb4efb`; head at read
  `efb3fe9c`) records `test -p memory-store ok`, `test -p daemon ok`, and
  "summarizer publisher fixtures" green.
- #873 replaces the publisher's session-wide `COUNT(*)` with a `MAX(sequence)`
  primary-key seek; `HistorySegmentSetGeneration::count` stays serialized as 0
  for rollback parsing.
- Publication entry: `publish_history_summarizer_chunk` at
  `crates/memory-store/src/lib.rs:12610` (`f2442b2f`).
- The drift test pins a firing, commits changed identities for a selected mid
  during the await, and asserts rejection without cooldown; the extension test
  commits an unrelated tail and asserts publication.
- #874's inventory measures the publication set fence at 3 rows and 51 VM
  steps at both H = 100 and H = 50,000 (description table).
- Late commits on #873 (`efb3fe9c`) and #874 (`08afec9d`, `cd596c15`)
  post-date the descriptions and touch append validation and the range-order
  check; they are on `main`, and the four witnesses above ran green with them
  in the #881 and #833 `-p memory-store` and `-p daemon` gates (read at
  `f2442b2f`). #833's D15 run reads the fence at 3 rows / 51 VM steps at N =
  10k and N = 1M.

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
  `f2442b2f`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 107, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

The producer selects messages 10 to 20; the host edits message 15 during the
model call. Publishing would store a summary of text the session no longer
holds.

## Timing windows and dependencies

The producer await, which spans a model call. Competing writers are the
transform pass, a recut, and another publication.

## What a test must construct

Pin a firing with a selected range, then during the await change one selected
identity, bump the epoch, or truncate and regrow to the same maximum. Assert
the publication is refused and no segment row is written. Run a tail extension
as a control and assert it publishes.

## Investigation log

### Q: Does the `MAX(sequence)` fence detect a truncate followed by regrowth?

- Sources examined: #873 description;
  `publish_rejects_a_firing_whose_set_was_truncated_and_regrown_to_the_same_maximum`
  at `lib.rs:22983`.
- Findings: The test exists and the description lists it; the epoch and row
  version fences reject the regrown set.
- Missing evidence: None at `f2442b2f`.
- Conclusion: resolved with answer: yes, pinned by the named test.

### Q: Does identity pruning (D12) weaken this fence?

- Sources examined: #833 ticket; comment 3 disposition ("extended to missing
  identities after pruning"); `crates/daemon/src/transform.rs:19708`.
- Findings: No. A pruned selected mid reads as drift:
  `a_prune_that_commits_first_fences_the_publication_out` publishes at the
  post-prune row version and gets `FenceRejected` with no segment written; it
  ran in #833's daemon gate (WP-P06).
- Missing evidence: None.
- Conclusion: resolved with answer: the fence rejects a missing identity.
