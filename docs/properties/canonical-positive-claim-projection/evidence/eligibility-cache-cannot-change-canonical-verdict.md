# eligibility-cache-cannot-change-canonical-verdict

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `final_use_is_judged_per_surface_from_current_canonical_policy`; `crates/daemon/tests/kernel_routes.rs` - `eligibility_verdicts_cover_every_class_and_cache_per_incarnation_and_tip`.

## Failure scenario

A cache answers a different verdict than the kernel would.

## Timing windows and dependencies

None.

## What a test must construct

A candidate list with every entry repeated.

## Investigation log

No open questions at authoring time.
