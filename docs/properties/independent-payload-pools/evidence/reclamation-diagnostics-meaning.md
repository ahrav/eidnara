# reclamation-diagnostics-meaning

## Discovery trigger

Diagnostics report outstanding return obligations, quarantined commitment, and actually released backing as distinct, consistently snapshotted quantities. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/retained.rs:342`
- `crates/host-runtime/src/ring_transport.rs:275`

Witness status: partial - the transport exposes `outstanding_returns` (`crates/shm-transport/src/backend/retained.rs:426`) and `PoolInventory` (`crates/shm-transport/src/backend/ring.rs:533`); the host's `reclamation.completed` counter still counts generation ends (`crates/host-runtime/src/ring_transport.rs:261`).

## Failure scenario

A counter read as released storage misleads operators about reclaimable capacity.

## Timing windows and dependencies

A held reader across a connection generation end.

## What a test must construct

A generation ends while a lease is live.

Situation markers that must fire independently of the safety check:

- `host.generation_ended_with_live_lease`

Check semantics: `always` - `outstanding_returns` counts live leases exactly, and the host counter's meaning is corrected without changing wire names.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548.
- Conclusion: unresolved, needs the named handoff.
