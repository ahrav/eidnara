# retrieved-content-cannot-upgrade-write-authority

## Discovery trigger

Specification 'Echo provenance and use authority': poisoned retrieved content never widens authorization, tool/edit capability, or canonical eligibility.

## Evidence trail

- `crates/kernel/src/slice/write.rs`: `insert_observation` and `correct_observation` refuse `uses_causality_namespace` specs and refuse to correct a reserved-kind row.
- `crates/kernel/tests/kernel_claim_facts.rs`: `forged_records_and_copied_strings_grant_nothing` attempts kind, observation-id, and object-id forgeries and a generic correction of a real record.

## Failure scenario

A generic writer accepting the causality kind would let any producer mint lineage; a generic correction would let it replace a real record.

## Timing windows and dependencies

None.

## What a test must construct

Attempt each forgery through `commit`; assert `InvalidInput` and no change to the class.

## Investigation log

No open questions at authoring time.
