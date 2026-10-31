# p6-failed-cheap-check-is-stale-and-handed-to-repair

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S3. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

The cheap-check match in `crates/kernel/src/applicability/checks.rs` evaluates `FileExists` at `:285` and `ConfigKey` at `:296` and reports `Symbol` and unrecognized kinds unsupported at `:328`. `unsupported_symbol_checks_are_uncertain_and_failed_checks_go_stale` (`crates/kernel/tests/kernel_applicability_engine.rs:283`) and `failed_check_appends_observation_event_and_job_in_one_commit` (`crates/kernel/tests/kernel_read_repair.rs:141`) cover the claim.

## Failure scenario

A failed check reports `Current`, or repair writes a partial set.

## Timing windows and dependencies

None.

## What a test must construct

A candidate whose cheap check fails against the checkout.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
