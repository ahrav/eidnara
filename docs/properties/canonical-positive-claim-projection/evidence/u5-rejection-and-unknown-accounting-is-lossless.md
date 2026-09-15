# u5-rejection-and-unknown-accounting-is-lossless

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `final_use_is_judged_per_surface_from_current_canonical_policy`.

## Failure scenario

Merged counts hide how many rejections were also Unknown.

## Timing windows and dependencies

None.

## What a test must construct

A rejected claim with Unknown lineage and a permitted claim with known lineage in one batch.

## Investigation log

No open questions at authoring time.
