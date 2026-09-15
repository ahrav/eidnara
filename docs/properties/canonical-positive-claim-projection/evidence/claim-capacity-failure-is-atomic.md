# claim-capacity-failure-is-atomic

## Discovery trigger

Specification 'Bounds, capabilities and stop conditions': bound rows, provenance references, and validation batches before expansion. Ticket #458: bound rows, bytes, decoded allocation, provenance, and affected-subject enumeration before expansion.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `claim_facts_as_of` returns `TooManyClaims` before opening a transaction when the request exceeds `max_claims`.
- `crates/kernel/src/claim_causality.rs`: `check_parents` refuses more than `MAX_DERIVATION_PARENTS` before any parent row is read; the refusal poisons the envelope through `guarded_typed`.
- `crates/kernel/tests/kernel_claim_facts.rs`: `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly` and the `too-many` case of `derived_reinjection_rests_on_exact_live_parents`.

## Failure scenario

A reader that truncated to the bound would return a partial inventory that looks complete; a writer that inserted the first N parents before refusing would leave a record whose dependencies disagree with its detail.

## Timing windows and dependencies

None.

## What a test must construct

Request `max_claims + 1` ids and assert the error; record a derivation with 17 parents and assert the commit fails with no new commit sequence.

## Investigation log

No open questions at authoring time.
