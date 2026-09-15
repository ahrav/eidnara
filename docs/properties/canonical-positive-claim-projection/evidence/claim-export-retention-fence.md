# claim-export-retention-fence

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/kernel/tests/kernel_source_holds.rs`, `crates/kernel/tests/kernel_source_export.rs`.

## Failure scenario

A rebuild reads descriptors whose bytes are gone and publishes a partial replacement.

## Timing windows and dependencies

Pruning between hold capture and export.

## What a test must construct

A released or expired hold.

## Investigation log

No open questions at authoring time.
