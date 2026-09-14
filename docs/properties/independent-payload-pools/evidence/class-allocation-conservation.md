# class-allocation-conservation

## Discovery trigger

Each class's free, reserved, and published counts sum to its block count; an ordinary bound takes the smallest fitting class and never spills; abort and underfill return the block; a short commit retains it (KTD1). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/pool.rs:273`
- `crates/shm-transport/src/backend/ring.rs:1133`
- `crates/shm-transport/src/backend/ring.rs:533`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2179`, `crates/shm-transport/src/backend/ring.rs:2208`, and `crates/shm-transport/src/backend/ring.rs:2243`.

## Failure scenario

A lost or double-counted block either strands capacity forever or hands one block to two frames.

## Timing windows and dependencies

Abort and underfill paths; a bound exactly at, one below, and one above each class boundary.

## What a test must construct

A class exhausted while a larger class has free blocks; an aborted reservation; an underfilled commit; a body of exactly `MAX_FRAME_BYTES` and of `MAX_FRAME_BYTES + 1`.

Situation markers that must fire independently of the safety check:

- `pool.class_exhausted_with_larger_class_free`
- `pool.reservation_aborted`
- `pool.commit_underfilled`
- `pool.maximum_body_reserved`

Check semantics: `always` - `PoolInventory::conserves` holds after every reserve, abort, commit, and reclaim, and `class_for` returns the smallest fitting class or `None`.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
