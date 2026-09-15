# environment-finalizer-confinement

## Discovery trigger

Finalizers and cleanup hooks own only their declared context: no ring call, allocator mismatch, unwind across C, or arbitrary N-API; uncertain cleanup quarantines rather than unmapping (KTD6). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `packages/shm-native/src/lib.rs:495`
- `crates/shm-transport/src/lease.rs:432`

Witness status: partial - `packages/shm-native/src/lib.rs:471` closes channels on the environment cleanup hook and `mem::forget`s alias-holding channels; the owned lease's drop is the only finalizer-adjacent return and reaches no N-API (`crates/shm-transport/src/backend/retained.rs:588`).

## Failure scenario

A finalizer calling N-API off-thread or unmapping under an alias is a crash or a use-after-unmap.

## Timing windows and dependencies

Environment teardown with aliases outstanding.

## What a test must construct

An environment exit while a channel holds a stranded alias.

Situation markers that must fire independently of the safety check:

- `native.environment_exit_with_aliases`

Check semantics: `always` - `cleanup_env` never calls `Ring` methods other than `enter_quarantine`, and a final drop never reaches the N-API boundary; no observer instruments that boundary in `shm-transport`, so the check is on the addon's detach-before-return path.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #550 for late-finalizer witnesses.
- Conclusion: unresolved, needs the named handoff.
