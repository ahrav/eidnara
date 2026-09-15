# owned-lease-thread-boundary

## Discovery trigger

`PayloadLease` is `Send`; `Ring`, `ProducerReservation`, and `LeaseSpan` are `!Send`; `Retained` is `Send + Sync` by explicit justification (R5, KTD4). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:25`
- `crates/shm-transport/src/backend/retained.rs:354`
- `crates/shm-transport/src/lease.rs:21`

Witness status: yes - compile-time: the `compile_fail` doctests at the top of crates/shm-transport/src/backend/ring.rs and the `assert_send::<PayloadLease>` in crates/shm-transport/src/lease.rs; runtime: `crates/shm-transport/src/lease.rs:573` and `crates/shm-transport/src/backend/ring.rs:3125`.

## Failure scenario

A `Send` `Ring` would let two threads drive one endpoint's non-atomic ledger.

## Timing windows and dependencies

None; type-level.

## What a test must construct

A build of the doctests.

Situation markers that must fire independently of the safety check:

- `lease.moved_across_threads`

Check semantics: `always` - the positive and negative `Send` assertions compile as written; no other `unsafe impl Send/Sync` exists in the crate.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
