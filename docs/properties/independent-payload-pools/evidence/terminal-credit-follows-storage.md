# terminal-credit-follows-storage

## Discovery trigger

One terminal credit is reserved before request admission and released only when its block returns or its generation retires, not when a callback completes. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/dispatch.rs:652`
- `crates/host-runtime/src/connection.rs:98`
- `crates/shm-transport/src/backend/ring.rs:1655`
- `crates/host-runtime/src/ring_transport.rs:1378`

Witness status: yes - `crates/host-runtime/src/ring_transport.rs:3441` publishes a terminal carrying a credit and shows the credit outstanding until `Ring::take_reclaimed` observes the block's return; `crates/host-runtime/tests/dispatch.rs:1649` admits 63 unsettled requests, refuses the 64th with `server_busy`/`terminal capacity exhausted` and zero dispatch while pending slots remain, then dispatches again only after the cancelled terminal's block is consumed.

## Failure scenario

A credit refunded on callback completion lets terminals exceed the reserved inventory.

## Timing windows and dependencies

Cancellation and foreign retention of a terminal block.

A peer return that lands between one pump's `take_reclaimed` scan and the same
pump's `try_reserve_in` puts the block on both the free list and the reclaim
list; the reservation pops it first, so the next terminal publishes into it.
`try_reserve_in` removes the block from the reclaim list at that point
(`crates/shm-transport/src/backend/ring.rs:996`), so the later drain settles
only the new publication's return.

## What a test must construct

A cancelled request whose terminal block is still held by the peer.

A returned block reused before the owner drains its return:
`crates/shm-transport/src/backend/ring.rs:2374` publishes, releases, publishes
again into the same block, and shows `take_reclaimed` reporting nothing until the
second publication is released; `crates/host-runtime/src/ring_transport.rs:3475`
does the same through `Publisher::pump` with the peer acting inside the publish
hook, and shows the credit on the reused block held until that release.

Situation markers that must fire independently of the safety check:

- `host.terminal_held_after_cancel`

Check semantics: `always` - the count of admitted requests never exceeds terminal credits, and a credit is released exactly once at the physical return point.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
