# private-decode-input-stability

## Discovery trigger

Rust decoding reads only stable private bytes: routed and channel-0 bodies are copied out of the lease before any parser sees them, and the lease is released after the last copy (KTD4). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/ring_transport.rs:762`
- `crates/host-runtime/src/frame_channel.rs:111`

Witness status: partial - the host still copies every body with `to_vec` before `InboundFrame::owned` (`crates/host-runtime/src/ring_transport.rs:762`); the owned raw-lease `InboundFrame` and channel-0 private copy belong to #548.

## Failure scenario

Decoding shared bytes would let a peer change a message under the parser.

## Timing windows and dependencies

A peer rewriting a published block during the copy.

## What a test must construct

A copy racing a peer write of the same block.

Situation markers that must fire independently of the safety check:

- `host.copy_races_peer_write`

Check semantics: `always` - no `InboundFrame` or control decoder holds a `LeaseSpan`; `lease.release()` precedes `deliver`.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548 owns the raw-lease `InboundFrame` conversion and the header/body consistency check.
- Conclusion: unresolved, needs the named handoff.
