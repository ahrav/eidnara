# descriptor-private-snapshot

## Discovery trigger

The receiver validates only a private copy of the four descriptor words taken once under the Acquire on `published`; no check rereads shared memory and no lease references the slot. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1453`
- `crates/shm-transport/src/descriptor.rs:222`
- `crates/shm-transport/src/backend/ring.rs:1501`

Witness status: partial - `crates/shm-transport/src/backend/ring.rs:2153` copies the four fields out under Miri and shows a later peer rewrite changes nothing the receiver holds; `crates/shm-transport/src/backend/ring.rs:2615` forges every field through the peer handle. No test races a rewrite against the copy itself.

## Failure scenario

A receiver that rereads the slot could validate one value and lease another, exposing bytes outside the block or a body longer than the block holds.

## Timing windows and dependencies

The window between the four relaxed loads and the compare-exchange on `consumed`, during which a peer may rewrite the slot.

## What a test must construct

A peer rewrite of a slot field after publication; the consumer must have observed `published` past that slot.

Situation markers that must fire independently of the safety check:

- `pool.slot_rewritten_after_publication`
- `pool.consumer_saw_published_sequence`

Check semantics: `always` - after `try_receive_inner` returns, the lease's block, generation, and body length equal the values `PoolDescriptor::from_untrusted` captured, whatever the slot holds now; the semantics are `always` because the property must hold on every receive, not only when a rewrite occurs.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: unsafe-review for the copy-out contract; invariant-test-review for the forged-field test's discriminating power.
- Conclusion: unresolved, needs the named handoff.
