# private-decode-input-stability

## Discovery trigger

Rust decoding reads only stable private bytes: routed and channel-0 bodies are copied out of the lease before any parser sees them, and the lease is released after the last copy (KTD4). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/frame_channel.rs:107`
- `crates/host-runtime/src/connection.rs:532`
- `crates/host-runtime/src/dispatch.rs:993`

Witness status: yes - `crates/host-runtime/src/ring_transport.rs:1747` shows the ring slot released once the body is private; `InboundFrame::into_private` (`crates/host-runtime/src/frame_channel.rs:107`) copies, releases the lease, then checks the copied length against the header, and `decode_control_frame` (`crates/host-runtime/src/connection.rs:532`) parses channel-0 bodies only from that private copy. Oversized channel-0 requests are refused before any lease (`crates/host-runtime/src/ring_transport.rs:929`).

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
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
