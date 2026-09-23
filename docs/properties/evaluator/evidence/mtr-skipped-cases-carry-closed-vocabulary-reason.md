# mtr-skipped-cases-carry-closed-vocabulary-reason

## Discovery trigger
Ticket #767: "Missing cutoff evidence, unavailable source, unsupported
runtime, and budget exhaustion retain distinct typed reasons and censoring
semantics. Missing explicit campaign settings refuse before execution."

## Evidence trail
- `crates/eval-core/src/campaign.rs` `SkipReason::MissingCutoffEvidence`,
  `UnsupportedReason::SourceUnavailable`,
  `UnsupportedReason::UnsupportedRuntime { family }`.
- `crates/eval-core/src/anchor.rs` `RealHistorySettings::validate`;
  `AnchorEntry::validate` refuses an id that is not one plain path component
  (`NotAPathComponent`).
- `crates/daemon/examples/eval_runner/anchor.rs`: `prepare` answers
  `source_unavailable` when the clone, the fetch, or a commit is missing;
  `run` answers `missing_cutoff_evidence` for a failed audit and
  `unsupported_runtime` for a Django task; settings are validated before
  the first clone.
- `crates/eval-core/tests/anchor.rs` `settings_refuse_before_execution_and_reasons_are_typed`: each
  refusal and each wire form.
- `crates/daemon/tests/eval_anchor.rs` `every_anchor_task_has_an_audit_a_proof_and_a_control_and_the_pilot_never_transfers`
  and `missing_settings_and_an_unaccepted_witness_refuse_before_execution`.

## Failure scenario
A task whose source is gone is recorded as a failed audit; a reader would
look for future knowledge that was never there.

## Timing windows and dependencies
None.

## What a test must construct
One entry per cause, and settings with each field missing in turn.

## Investigation log
### Q: Where does budget exhaustion sit?
- Sources examined: `Terminal::Censored`; the shell's control deadline,
  `hidden_terminal`, `a_control_past_its_deadline_is_censored_with_its_trace_and_a_failed_control_refuses`.
- Findings: a control past its deadline is `Censored { hard_deadline_ms }`
  and is not graded; `classify_control` treats it as eligible, never
  memorized, unless its kept trace shows repository access or a future
  answer.
- Missing evidence: none.
- Conclusion: resolved with answer.
