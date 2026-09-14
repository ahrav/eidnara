# terminal-credit-follows-storage

## Discovery trigger

One terminal credit is reserved before request admission and released only when its block returns or its generation retires, not when a callback completes. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/pool.rs:40`
- `crates/shm-transport/src/backend/ring.rs:973`

Witness status: not yet - the terminal-credit and dedicated encoding reservation are #548's implementation over the `Inventory::Terminal` class this transport provides.

## Failure scenario

A credit refunded on callback completion lets terminals exceed the reserved inventory.

## Timing windows and dependencies

Cancellation and foreign retention of a terminal block.

## What a test must construct

A cancelled request whose terminal block is still held by the peer.

Situation markers that must fire independently of the safety check:

- `host.terminal_held_after_cancel`

Check semantics: `always` - the count of admitted requests never exceeds terminal credits, and a credit is released exactly once at the physical return point.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: not yet at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548.
- Conclusion: unresolved, needs the named handoff.
