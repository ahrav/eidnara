# guarded-callbacks-enforce-current-authority

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Callback setup and statement reauthorization cost can motivate caching. The
contract to preserve is authorization at use, not a mandatory preparation rate.

## Evidence trail

- [storage/lib.rs:195-209][lock] acquires a synchronous mutex and rejects
  same-thread reentry. [229-245][read] installs a read-only callback scope.
- [290-316][write] prechecks and claims the fence, pins durability, and commits
  only after the guarded callback and successful scope release.
- [624-705][scope] checks temp shadows, installs the authorizer, compares
  infrastructure names to detect rename effects, and restores scope state.
- [487-498][cache] documents prepared-statement expiry on scope installation.
- [memory-store/lib.rs:5563-5586][facade] installs caller/domain/route scopes
  inside the connection-locked callback; [5999-6025][notes] does the same for
  note callers. [4772-4823][scope-owners] records thread ownership and restoration.

## Failure scenario

A statement cached under a writable or different facade scope runs after the
scope changes without equivalent authorization. A maintenance-created shadow
redirects a later unqualified query, or unwind leaves the next caller's scope
incorrect. These are effect-level failures, even if statement caching succeeds.

## Timing windows and dependencies

Maintenance can change connection-local schema between independent callbacks.
Installing a non-NULL authorizer expires statements; clearing is not assumed
to do so. The optimized mechanism must account for changed authority regardless
of whether SQLite prepares again. Nonfacade calls legitimately lack facade scope.

## What a test must construct

Alternate read, fenced write, and maintenance calls; warm cached SQL, then
change authority. Include preexisting shadows, infrastructure rename attempts,
callback errors, panic, and contended facade callers. Compare results, refusals,
durable effects, and the next call's restored state against the baseline.
[Existing checks](../existing-checks.md#guarded-store) remain unaudited.
No authority-transition experiment runs; this property is not exercised.

## Investigation log

### Q: What makes reduced setup safe after authority or maintenance changes?

- Sources examined: [Scope installation][scope], [cache documentation][cache],
  and [facade scope placement][facade].
- Findings: The baseline re-establishes scope per callback and uses current
  same-thread facade ownership. Caching alone does not replace that contract.
- Missing evidence: A proposed invalidation mechanism and discriminating tests
  are not supplied.
- Conclusion: Observable authority is the resolved requirement; setup elision
  remains unresolved. No schema ledger or weakened durability is authorized.

[lock]: ../../../../crates/storage/src/lib.rs#L195-L209
[read]: ../../../../crates/storage/src/lib.rs#L229-L245
[write]: ../../../../crates/storage/src/lib.rs#L290-L316
[scope]: ../../../../crates/storage/src/lib.rs#L624-L705
[cache]: ../../../../crates/storage/src/lib.rs#L487-L498
[facade]: ../../../../crates/memory-store/src/lib.rs#L5563-L5586
[notes]: ../../../../crates/memory-store/src/lib.rs#L5999-L6025
[scope-owners]: ../../../../crates/memory-store/src/lib.rs#L4772-L4823
