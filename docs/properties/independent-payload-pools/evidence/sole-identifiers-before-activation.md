# sole-identifiers-before-activation

## Discovery trigger

Only descriptor schema 4, layout version 4, and profile `host-payload-pool-v1` are accepted; any other identifier, an eventfd or datagram doorbell, or a pool with traffic in flight fails before application traffic (R8, KTD8). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:190`
- `crates/shm-transport/src/backend/ring.rs:726`
- `crates/host-runtime/src/ring_transport.rs:1510`
- `packages/shm-native/src/lib.rs:305`

Witness status: yes - `crates/shm-transport/tests/contract.rs:167`, `crates/shm-transport/src/backend/ring.rs:3134`, `crates/shm-transport/tests/profile.rs:261`, and `stale_wire_or_descriptor_schema_is_invalid_identity` in `crates/host-runtime/src/setup_socket.rs`.

## Failure scenario

An accepted stale identifier would decode another layout's bytes as this one's.

## Timing windows and dependencies

None; this is fail-closed identity checking.

## What a test must construct

A grant with layout version 3; a setup message with schema 3; an eventfd in a doorbell position; a non-fresh pool at attach.

Situation markers that must fire independently of the safety check:

- `setup.stale_identifier_presented`
- `setup.wrong_doorbell_type_presented`
- `setup.non_fresh_pool_presented`

Check semantics: `always` - every mismatch path returns an error before `Mapping::attach` or before `activate` commits, and no code path decodes another layout.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
