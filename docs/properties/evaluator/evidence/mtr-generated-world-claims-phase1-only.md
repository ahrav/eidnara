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
- `crates/daemon/examples/eval_runner/anchor.rs` `control`: the workspace sits
  outside the private root that the containment covers with an empty tmpfs;
  an announced call that climbs out of the workspace or names the private root
  is repository access; `run_bounded` keeps the trace printed before a
  deadline kill; the terminal is `hidden_terminal`. `run` freezes
  `Config::transfer_criterion` into the analysis family and derives claims
  with `AnalysisFamily::claim_class`, one per `ProviderProfile`, never keyed
  by a rendered string.
- `crates/daemon/tests/eval_anchor.rs` `a_memorizing_provider_is_excluded_for_its_pair_and_seeded_contamination_is_detected`:
  the memorizing script passes every control from the statement alone, every
  test in each hidden file passing, every graded task is excluded for both
  pairs with its reason, and every pilot row is skipped; the contaminated script's relative repository read is
  detected and denied (the memorized fix it would have overwritten still
  passes) and its pull-request citation is detected; the second run's control
  script and criterion change its run id and family digest.
- `crates/daemon/tests/eval_anchor.rs` `a_control_past_its_deadline_is_censored_with_its_trace_and_a_failed_control_refuses`:
  a control that announces a repository read and stalls is `Censored` with
  the read kept and excluded; a control that exits non-zero refuses the run.

## Failure scenario
A provider that has seen the fix upstream solves the task from the issue
text; counting it would measure memorization as transfer. A control that can
read the clone or snapshot, or whose trace is dropped at its deadline, is
contaminated without the report showing it.

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
