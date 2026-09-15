# bound-project-scope-cannot-be-widened-by-candidate

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/kernel/tests/kernel_eligibility.rs`.

## Failure scenario

A request bound to one project delivers another project's claims.

## Timing windows and dependencies

None.

## What a test must construct

A claim scoped to another project.

## Investigation log

No open questions at authoring time.
