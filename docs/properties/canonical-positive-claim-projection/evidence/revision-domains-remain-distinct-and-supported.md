# revision-domains-remain-distinct-and-supported

## Discovery trigger

Specification 'Canonical facts and identity': source revision, policy revision, and canonical snapshot authority are distinct; render-input digests cannot fence them.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `ClaimFacts.object.source_revision`, each `AdmissionFacts.policy_revision`, and `ClaimFactsSnapshot.known_as_of` are separate fields; the module doc names them as the revision domains.
- `crates/kernel/src/claim_causality.rs`: `CLAIM_CAUSALITY_DETAIL_VERSION` is its own domain; a mismatch yields `Unknown(UnsupportedVersion)` without affecting the other facts.
- `crates/kernel/src/admission.rs`: `decided_row` fails closed when `policy_revision > POLICY_REVISION`.
- `crates/kernel/tests/kernel_claim_facts.rs`: field assertions in the snapshot test and the version branch of the replay test.

## Failure scenario

A reader returning one combined revision would let a policy bump look like a new source revision to downstream tombstoning.

## Timing windows and dependencies

None.

## What a test must construct

Assert the four fields independently; rewrite the causality version and assert the other facts are still returned.

## Investigation log

No open questions at authoring time.
