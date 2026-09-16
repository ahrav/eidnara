# supporting-authority-is-preserved-not-recomputed

## Discovery trigger

Specification 'Canonical facts and identity': supporting approval and merely cited approval are distinct; missing required fields are not defaults that imply approval.

## Evidence trail

- `crates/kernel/src/admission.rs`: `write_admission` stores the supporting approval in `approval_object_id` and the cited one only in the change-event audit.
- `crates/kernel/src/claim_facts.rs`: `load_admission` copies `approval_object_id` and evaluates `approval_chain_valid_at_snapshot_sql` for it; nothing in the reader consults the cited approval.
- `crates/kernel/tests/kernel_claim_facts.rs`: `supporting_approval_is_copied_with_its_validity_at_the_snapshot` admits under `approval`, revokes it, compares both snapshots with the writer's reported decisions, and rewrites a row out of band to read `valid_at_snapshot == false`.

## Failure scenario

Reading the cited approval from the audit JSON and treating it as support would grant elevated maturity the evaluator never derived.

## Timing windows and dependencies

Approval revoked between the admission and the snapshot.

## What a test must construct

Admit with a valid approval, revoke it, read facts at both snapshots, assert `valid_at_snapshot` flips while `object_id` stays.

## Investigation log

No open questions at authoring time.
