# ceil-arm-leaves-claim-class-unchanged

## Discovery trigger
Parent specification #1119 C14, C15, D3, OQ7-3 (resolved in #1123: a declaration block, not new
`ArmKind` variants); Part B record `ceil-arm-leaves-claim-class-unchanged`.

## Evidence trail
- `crates/eval-core/src/claim.rs:138` `derive_claim_class(provenance,
  anchor_set, criterion)`.
- `crates/eval-core/src/report.rs:64` `CampaignGates::of` reads the ledger,
  the rate ceilings, the family, and the arm rates; `:903` the marginal bounds
  read the ledger's aged and fresh arms.
- `crates/eval-core/tests/report.rs::a_declared_ceiling_leaves_the_claim_marginals_rates_and_gates_unchanged`.

## Failure scenario
Ceiling samples in the sample ledger would move the attempted-sample
denominators of the indeterminate and censoring gates; a ceiling's injected
anchors passed as `anchor_set` would derive `transfer`.

## Timing windows and dependencies
None.

## What a test must construct
A run with a valid `Transfer` ceiling beside a `GeneratedPhase1` derivation.

Legal witness: `derive_claim_class(Generated, None, None)` is
`GeneratedPhase1` with and without a ceiling. Smallest falsifier: a ceiling's
injected anchor set passed as `anchor_set`, yielding `Transfer`.

## Investigation log
### Q: Do ceiling figures appear in the report?
- Sources examined: the report type
- Findings: the report carries declarations only; figures come from the
  runner's ceiling campaign (#1007)
- Missing evidence: none.
- Conclusion: resolved with answer - a figure travels with its declaration
  when the runner reports one
