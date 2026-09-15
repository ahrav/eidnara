# stale-projection-cannot-authorize-current-use

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `final_use_is_judged_per_surface_from_current_canonical_policy`.

## Failure scenario

A selected claim is delivered after the kernel revoked it.

## Timing windows and dependencies

The window between selection and handoff.

## What a test must construct

Approve-then-quarantine and correction landing between two validations.

## Investigation log

No open questions at authoring time.
