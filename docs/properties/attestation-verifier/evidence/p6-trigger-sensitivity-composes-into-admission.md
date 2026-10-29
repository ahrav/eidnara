# p6-trigger-sensitivity-composes-into-admission

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`validate_trigger` (`crates/kernel/src/admission.rs:2807`) returns the observation's class folded with its backing evidence's class. `record_admission` (`:998`) passes `facts.sensitivity.max(trigger_sensitivity)` to the evaluator after the call at `:1662`. `surface_visibility` (`:4221`) hides `Sensitive` on `AutoInject` and `AutoSearch`.

## Failure scenario

A `Normal` observation over `Sensitive` evidence admits content below the class of the material that supports it.

## Timing windows and dependencies

None.

## What a test must construct

A trigger observation at `Normal` citing `Sensitive` evidence, and a subject at `Normal`.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
