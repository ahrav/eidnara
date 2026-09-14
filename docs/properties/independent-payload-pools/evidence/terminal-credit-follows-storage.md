# terminal-credit-follows-storage

## Discovery trigger

One terminal credit is reserved before request admission and released only when its block returns or its generation retires, not when a callback completes. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/dispatch.rs:652`
- `crates/host-runtime/src/connection.rs:98`
- `crates/shm-transport/src/backend/ring.rs:1504`
- `crates/host-runtime/src/ring_transport.rs:1221`

Witness status: yes - `crates/host-runtime/src/ring_transport.rs:2597` publishes a terminal carrying a credit and shows the credit outstanding until `Ring::take_reclaimed` observes the block's return; `crates/host-runtime/tests/dispatch.rs:1571` admits 63 unsettled requests, refuses the 64th with `server_busy`/`terminal capacity exhausted` and zero dispatch while pending slots remain, then dispatches again only after the cancelled terminal's block is consumed.

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
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
