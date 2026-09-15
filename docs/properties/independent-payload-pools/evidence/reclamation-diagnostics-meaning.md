# reclamation-diagnostics-meaning

## Discovery trigger

Diagnostics report outstanding return obligations, quarantined commitment, and actually released backing as distinct quantities, each sampled independently rather than as one atomic snapshot. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/retained.rs:342`
- `crates/host-runtime/src/ring_transport.rs:262`
- `crates/host-runtime/src/ring_transport.rs:378`

Witness status: yes - `crates/host-runtime/src/ring_transport.rs:3855` takes one snapshot of live backings, outstanding leases, and released backing bytes while the endpoint runs and again after it ends, and shows `reclamation.completed` advancing for the generation end without advancing released backing; `RingTransport::return_snapshot` (`crates/host-runtime/src/ring_transport.rs:262`) reads both quantities under one lock and `diagnostics()` reports `reclamation.meaning`, `returns`, and `exhaustion.by_resource` as distinct objects under the existing wire names.

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
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
