# p6-policy-digest-pins-admission-policy

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`POLICY_REVISION` (`crates/kernel/src/admission.rs:20`) names the policy. `policy_revision_matches_policy_content` (`:5931`) and `the_served_fold_is_under_the_serving_digest` (`:5900`) hash the regions between `policy-digest` markers. `newer_policy_revision_fails_closed_while_older_still_serves` (`crates/kernel/tests/kernel_admission.rs:3371`) covers a stored row at a newer revision. This ticket leaves `evaluate_admission` untouched.

## Failure scenario

An evaluator change ships without a revision bump.

## Timing windows and dependencies

None.

## What a test must construct

An edit inside the evaluator region.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
