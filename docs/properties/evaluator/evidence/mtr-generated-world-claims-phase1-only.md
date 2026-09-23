# mtr-generated-world-claims-phase1-only

## Discovery trigger
Parent specification C-VAL: "Generated worlds carry Phase-1 claims only.
`claim_class == transfer` requires the real-history anchor set plus a frozen,
approved transfer criterion recorded in the analysis-family digest; the
20-task pilot never suffices alone. A per-task no-repository control marks
memorized tasks and excludes them from transfer claims for that
provider/model pair."

## Evidence trail
- `crates/eval-core/src/claim.rs` `derive_claim_class`.
- `crates/eval-core/src/anchor.rs` `classify_control` (memorized, repository
  access, future answer, not comparable) and `anchor_set` (valid, residue,
  cutoff invalid per pair).
- `crates/eval-core/tests/anchor.rs` `the_pilot_alone_never_transfers_and_exclusions_keep_their_accounting`:
  20 rows kept with 17 eligible; the pilot derives `generated_phase1` with
  `AnchorSetIsPilot`; a transfer-role set with one memorized task does not
  transfer for that pair and does once the control is eligible; no criterion
  refuses.
- `crates/daemon/tests/eval_anchor.rs` `a_memorizing_provider_is_excluded_for_its_pair_and_seeded_contamination_is_detected`:
  the memorizing script passes every control from the statement alone and
  every task is excluded for both pairs with its reason; the contaminated
  script's repository read and pull-request citation are detected.

## Failure scenario
A provider that has seen the fix upstream solves the task from the issue
text; counting it would measure memorization as transfer.

## Timing windows and dependencies
None.

## What a test must construct
Controls that pass, that reach the repository, and that name the future.

## Investigation log
### Q: Can a run ever derive `transfer` today?
- Sources examined: the shell's `role`.
- Findings: the shell fixes the role at `pilot`; a transfer-role set needs the
  full anchor set, which no corpus here holds.
- Missing evidence: the full anchor set.
- Conclusion: resolved with answer - not yet, by design.
