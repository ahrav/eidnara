# candidate-validation-preserves-surface-policy

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `final_use_is_judged_per_surface_from_current_canonical_policy`.

## Failure scenario

A batch `Ok` read as permission injects a claim the policy only allows on explicit search.

## Timing windows and dependencies

None.

## What a test must construct

Claims admitted with an `ExplicitLabeled` visibility row, validated on `ExplicitSearch` and `AutoInject`.

## Investigation log

No open questions at authoring time.
