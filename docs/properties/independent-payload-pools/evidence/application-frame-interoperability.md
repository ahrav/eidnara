# application-frame-interoperability

## Discovery trigger

The 21-byte header, JSON bodies, direct serializers, send outcomes, and the exact 67,108,864-byte maximum are unchanged; both directions accept one maximum frame on an admitted connection and refuse maximum-plus-one (R7). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/pool.rs:310`
- `crates/shm-transport/src/profile.rs:676`
- `crates/shm-transport/src/descriptor.rs:26`

Witness status: yes - `crates/shm-transport/tests/ring.rs:74` and `crates/shm-transport/src/backend/ring.rs:2420`; application vectors stay frozen in `crates/host-runtime/tests/protocol_vectors.rs`.

## Failure scenario

A smaller effective maximum would break the interoperability promise of the wire contract.

## Timing windows and dependencies

None; boundary values are the enabling state.

## What a test must construct

A body of exactly `MAX_FRAME_BYTES` in each direction on an otherwise empty connection.

Situation markers that must fire independently of the safety check:

- `pool.maximum_frame_each_direction`
- `pool.zero_body_frame`

Check semantics: `always` - every published body of length `n <= MAX_FRAME_BYTES` is received with identical bytes and header, and `MAX_FRAME_BYTES + 1` is `BoundExceedsClass` before any block is taken.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
