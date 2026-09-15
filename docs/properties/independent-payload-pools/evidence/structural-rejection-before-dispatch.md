# structural-rejection-before-dispatch

## Discovery trigger

A structurally illegal descriptor, header, or body length closes the generation before any application dispatch and exposes no byte. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1346`
- `crates/host-runtime/src/ring_transport.rs:947`

Witness status: partial - `crates/shm-transport/src/backend/ring.rs:2397` covers descriptor and header structure; host header validation stays in `validate_inbound_header` tests in `crates/host-runtime/src/frame_channel.rs`.

## Failure scenario

Dispatching a structurally illegal frame would let a peer steer application decoding with unchecked lengths.

## Timing windows and dependencies

Header/body mismatch written into the block; oversized declared length.

## What a test must construct

A block header whose declared length differs from the descriptor's body length.

Situation markers that must fire independently of the safety check:

- `pool.header_body_mismatch_in_block`
- `pool.oversized_body_declared`

Check semantics: `always` - `try_receive` returns `Err` and quarantines for every validation failure, and `receive_one` maps a header failure to `ReadClose::Corrupt` before delivering an `InboundEvent`.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none.
- Conclusion: unresolved, needs the named handoff.
