# claims-block-failure-never-fails-publish

## Discovery trigger

Specification #835 Property Catalog record
`claims-block-failure-never-fails-publish`, derived by the invariant-modeling
pass at `265df096`, and constraint C-5 (fail-open): a missing or unparseable
claims block leaves every segment with an empty claims array and publishes.
D-4 gives the claims block its own parse so a claims fault cannot reach the
facts outcome. Milestone ticket #838 carries it as the first acceptance
criterion: no block, a truncated block, and a 9-claim block publish the same
segments, advance `unprocessed_from`, and leave the facts outcome unchanged.

## Evidence trail

`crates/daemon/src/history_summarizer_validate.rs::parse_claims` returns a
triple of block presence, candidates, and an optional `ExtractionFailure`. It
never returns an error. A tag without its partner, a second block, or
material inside the block that is no `<claim>` element yields
`MalformedClaims` with no candidates.

`crates/daemon/src/history_summarizer_validate.rs::validate_history_summarizer_output`
computes `claims_outcome` after the discard-last step, the forward-progress
check, and the facts verdict. It maps no block to `NotRequested`, a syntax
failure to `Rejected`, and otherwise calls
`crates/daemon/src/history_summarizer_citations.rs::check_claim_set`, which
always returns `Accepted` with counts. The only effect on `history_segments`
is assigning each segment's `claims`. The `?` returns in the function all sit
before this point and concern segments, not claims.

`crates/daemon/src/history_summarizer_citations_golden.rs::a_claims_block_never_changes_what_publishes_or_the_facts_outcome`
validates golden case 1 with no block, a truncated block, a block of ten
claims over nine keys, two blocks, and stray text inside a block. It asserts
the outcomes `NotRequested`, `Rejected { MalformedClaims }`, and
`Accepted { kept: 8, dropped: 2, anchor_missing: 8 }`, and that each chunk,
with claims and `claims_outcome` cleared, equals the no-block chunk: two
segments, `unprocessed_from` 4, and `extraction` `Accepted { count: 1 }`.

`crates/daemon/src/history_summarizer.rs::tests::nonadmission_facts_survive_later_firings_failures_and_reopen`
publishes firing 2 with an empty facts block and a claims block through the
store. It asserts the publish succeeds, the floor reaches 6, no nonadmission
is recorded, and the last stored segment holds only the kept claim `k.v`.

## Failure scenario

A model emits a truncated `<claims>` block at the length cap. If the claims
parse shared the facts parse or returned an error, the chunk would fail
validation or its facts would read as `Rejected`, the publication floor
would stall, and the summarizer would refire the same range.

## Timing windows and dependencies

None. Validation is a pure function of the output text, the chunk, and the
prior ranges. The publish transaction consumes the validated chunk and does
not read `claims_outcome`.

## What a test must construct

One valid two-segment output with one accepted fact, then the same output
with each claims fault: no block, `<claims>` without its close, two blocks,
stray text beside a claim, and more than eight claims. Compare each validated
chunk to the no-block chunk after clearing claims and `claims_outcome`. One
publish through the store with a claims block must advance the floor.

## Investigation log

### Q: Can a claims fault return an error from validation?

- Sources examined: `parse_claims`, `validate_history_summarizer_output`,
  `check_claim_set`.
- Findings: `parse_claims` has no `Result`. `check_claim_set` has no
  `Result` and its only verdict is `Accepted`. The claims step in the
  validator has no `?` and no early return.
- Missing evidence: none.
- Conclusion: resolved with answer; no path from the claims block reaches
  the validator's error return.

### Q: Does a claims fault change the facts outcome?

- Sources examined: the validator's facts branch, the golden test.
- Findings: the facts verdict is computed from `fact_syntax_failure` and
  `check_fact_set` before the claims step and never reads claims state. The
  golden test asserts equal `extraction` across all five faulted chunks.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the named tests pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p memory-store -p daemon
  --all-features --locked` with a filter naming the tests above, run at
  `c38af85a`.
- Findings: both tests pass, with 39 of 39 selected tests passing.
- Missing evidence: none.
- Conclusion: resolved with answer.
