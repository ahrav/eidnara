# direct-serialization-commit-boundary

## Discovery trigger

A direct serializer runs only into a reserved block whose span is the logical body bound, is never consumed without a reservation, and a short result keeps its block (KTD1). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1742`
- `crates/host-runtime/src/ring_transport.rs:1432`

Witness status: yes - `crates/host-runtime/src/ring_transport.rs:3498` counts serializer invocations: zero while the class is exhausted through retirement, exactly one once a block is reserved; `crates/host-runtime/src/ring_transport.rs:1392` serializes through `ReservationWriter` only after reservation, and `crates/shm-transport/src/backend/ring.rs:2391` covers abort and short commit.

## Failure scenario

Serializing into class slack or without a reservation would write bytes no descriptor accounts for.

## Timing windows and dependencies

Reservation failure before serialization.

## What a test must construct

`reserve_until` returning `Deadline` with a `DirectFrame` queued.

Situation markers that must fire independently of the safety check:

- `host.direct_frame_unreserved_on_deadline`

Check semantics: `always` - `ProducerReservation::capacity()` equals the caller's bound, `segment(0)` has that length, and an unreserved `DirectFrame` is never invoked.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
