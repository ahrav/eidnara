# echo-classification-requires-canonical-causality

## Discovery trigger

Specification 'Echo provenance and use authority': `DirectObservation` and `DerivedReinjection` require canonical causal evidence; missing lineage is `Unknown`, never inferred from text or role.

## Evidence trail

- `crates/kernel/src/claim_causality.rs`: `check_acquisition` requires a live `evidence_meta` row, equal digest, and exact retention; `check_parents` requires live registered parents at the stated revision.
- `crates/kernel/src/claim_causality.rs`: `uses_causality_namespace` is checked by `insert_observation` and `correct_observation` in `crates/kernel/src/slice/write.rs`.
- `crates/kernel/src/claim_causality.rs`: `causal_class_at` only consults rows with `observation_kind = claim_causality` and `source_kind = claim_causality`.
- `crates/kernel/tests/kernel_claim_facts.rs`: `forged_records_and_copied_strings_grant_nothing`, `direct_observation_rests_on_live_exact_evidence`, `derived_reinjection_rests_on_exact_live_parents`.

## Failure scenario

A look-alike observation carrying `direct_observation` text and a daemon producer string would be classified as genuine, letting reinjected content claim independent support.

## Timing windows and dependencies

Evidence retired after the record; parent retired after the record; record retired.

## What a test must construct

Every refusal variant of `ClaimCausalityError` through a real commit; a look-alike under another kind; retire the record and evidence in order and read snapshots before and after each.

## Investigation log

No open questions at authoring time.
