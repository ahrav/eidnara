# wit-serializer-requires-claim-boundary-and-refuses-forbidden-claims

## Discovery trigger
Parent specification acceptance: "Every report and witness carries the
`claim-boundary/v1` block with the four exclusions; a report missing the
block or containing a forbidden claim phrase is refused."

## Evidence trail
- `crates/eval-core/src/manifest.rs` `CLAIM_BOUNDARY_EXCLUSIONS` and
  `ClaimBoundary::pinned`.
- `crates/eval-core/src/witness.rs:122` mismatch refusal; `:243`
  `check_claims` walks every string leaf outside `claim_boundary`.
- `crates/eval-core/tests/witness.rs:239` a popped exclusion and an oracle name naming live-model
  quality are refused with the path.

## Failure scenario
A witness's oracle is named "proves live-model quality"; without the scan the
phrase ships beside the block that excludes it.

## Timing windows and dependencies
None.

## What a test must construct
A package with the phrase in a free-text field.

## Investigation log
### Q: Which phrases are forbidden?
- Sources examined: parent #749; `CLAIM_BOUNDARY_EXCLUSIONS`.
- Findings: the four exclusion texts themselves, matched case-insensitively.
- Missing evidence: none.
- Conclusion: resolved with answer.
