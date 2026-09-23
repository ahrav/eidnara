# wit-serializer-requires-claim-boundary-and-refuses-forbidden-claims

## Discovery trigger
Parent specification acceptance: "Every report and witness carries the
`claim-boundary/v1` block with the four exclusions; a report missing the
block or containing a forbidden claim phrase is refused."

## Evidence trail
- `crates/eval-core/src/manifest.rs` `CLAIM_BOUNDARY_EXCLUSIONS` and
  `ClaimBoundary::pinned`.
- `crates/eval-core/src/witness.rs` `validate` refuses the mismatch;
  `check_claims` walks every string leaf outside the root `claim_boundary`
  key and names the first hit's path.
- `crates/eval-core/tests/witness.rs` `the_serializer_requires_the_verbatim_claim_boundary_and_rejects_forbidden_claims`:
  a popped exclusion and an oracle name naming live-model quality are
  refused through `serialize`; a capitalized "Scheduler-Order Independence"
  in the coverage array is refused at `/original/coverage[0]`.

## Failure scenario
A witness's oracle is named "proves live-model quality"; without the scan the
phrase ships beside the block that excludes it.

## Timing windows and dependencies
None.

## What a test must construct
A package with the phrase in a free-text field and in an array leaf.

## Investigation log
### Q: Which phrases are forbidden?
- Sources examined: parent #749; `CLAIM_BOUNDARY_EXCLUSIONS`.
- Findings: the four exclusion texts themselves, matched case-insensitively
  in ASCII.
- Missing evidence: none.
- Conclusion: resolved with answer.
### Q: Should separators and Unicode be normalized before matching?
- Sources examined: `check_claims`.
- Findings: no normalization beyond ASCII lowercasing.
- Missing evidence: a maintainer decision on the phrase grammar.
- Conclusion: unresolved, needs human input.
