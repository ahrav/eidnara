# reserved-publication-order

## Discovery trigger

Cross-class bypass preserves increasing consumer Request correlations, ordinary FIFO, per-stream data before terminal, and drain before Goodbye; a channel-0 Request is not a bypass control. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- docs/payload-pool-protocol.md section 11
- `docs/host-wire-protocol.md:314`
- `crates/host-runtime/src/ring_transport.rs:995`
- `crates/host-runtime/src/ring_transport.rs:1019`

Witness status: partial - `crates/host-runtime/src/ring_transport.rs:2546` checks the host publisher: a blocked ordinary head lets an eligible Ping and an unrelated terminal through, a terminal whose stream prefix is blocked waits, and Goodbye waits for every earlier frame; `crates/host-runtime/src/ring_transport.rs:2703` pins that a channel-0 Request is never a bypass control. The Rust client keeps `Cancel` and `Goodbye` behind the requests they govern on the data lane and lets only `Pong` bypass (`crates/host-runtime/src/client.rs:2805`); `a_cancel_stays_behind_the_request_it_governs` and `cancels_cannot_exhaust_the_pong_reserve` in crates/host-runtime/src/client.rs cover it, and `RingClientEndpoint::try_send_bounded` (`crates/host-runtime/src/ring_transport.rs:1399`) classifies each client frame's inventory with the same `inventory_for`. The native publisher belongs to #550.

## Failure scenario

A reordered terminal or correlation breaks the application contract.

## Timing windows and dependencies

A blocked ordinary ticket first in queue with eligible Ping and unrelated terminal behind it.

## What a test must construct

Ordinary exhaustion with a Ping and a terminal queued behind a data frame.

Situation markers that must fire independently of the safety check:

- `host.bypass_eligible_frame_behind_blocked_data`

Check semantics: `always` - the sequence of published headers per direction satisfies the four order predicates.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #550 for the native publisher.
- Conclusion: unresolved, needs the named handoff.
