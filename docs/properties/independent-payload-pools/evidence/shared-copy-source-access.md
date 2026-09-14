# shared-copy-source-access

## Discovery trigger

Every access to shared bytes is a fixed-width atomic or an `AccessShape` copy; no `&[u8]` over the arena exists; a copy stabilizes destination bytes only, and consumers decode from private copies (R6, KTD4). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/lease.rs:142`
- `crates/shm-transport/src/lease.rs:188`
- docs/payload-pool-protocol.md section 10

Witness status: partial - `crates/shm-transport/src/lease.rs:650` and `crates/shm-transport/src/lease.rs:756` under Miri prove same-shape access; cross-process hostile writers are not provable here (recorded limitation).

## Failure scenario

A Rust slice over peer-writable memory is undefined behavior the moment the peer writes.

## Timing windows and dependencies

A same-shape concurrent writer; a shifted overlapping writer is outside the contract and documented.

## What a test must construct

A writer thread storing through `copy_in` while a span is read.

Situation markers that must fire independently of the safety check:

- `lease.concurrent_same_shape_writer`
- `lease.copy_at_every_alignment`

Check semantics: `always` - `rg` over `crates/shm-transport/src` finds no `slice::from_raw_parts` over arena memory, and every raw pointer escape is one of `ProducerReservation::segment` and `PayloadLease::body`.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: safe-over-unsafe then unsafe-review for the shifted-overlap and JavaScript-writer exclusions.
- Conclusion: unresolved, needs the named handoff.
