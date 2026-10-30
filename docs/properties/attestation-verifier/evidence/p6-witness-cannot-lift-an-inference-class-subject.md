# p6-witness-cannot-lift-an-inference-class-subject

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`evaluate_admission` (`crates/kernel/src/admission.rs:280`) refuses with `AdmissionPolicy` when the prior admission's source or taint class differs from the request's (`:284`). `automatic_ceiling` (`:4302`) maps `AssistantInference`, `MemoryClassifierInference`, and the other untrusted taints to `Candidate`; only `(TrustedLocalCode | TrustedToolResult, CurrentCode | CurrentTest | CurrentConfig)` and the explicit-user pairs reach `Verified`. The tests are `inference_ceiling_requires_authority_and_lifts_only_to_event_target` (`:4942`) and `every_legal_source_taint_pair_enforces_its_automatic_ceiling` (`:4720`). This is why the verifier writes a new subject (C1).

## Failure scenario

A witness event on the model decision lifts it to `Verified`, so the proposing model certifies itself.

## Timing windows and dependencies

None.

## What a test must construct

A `(ModelInference, AssistantInference)` subject with a validated `CodeObserved` trigger, and a second admission of that subject under trusted classes.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
