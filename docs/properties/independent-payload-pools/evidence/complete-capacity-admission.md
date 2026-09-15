# complete-capacity-admission

## Discovery trigger

Full capacity is charged before activation from the complete layout: mapping bytes, ledger bytes, descriptors, blocks, mappings, file descriptors, wake handles, and the instance; `affordable_connections` divides the byte ceiling by the complete committed charge (KTD7). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/profile.rs:166`
- `crates/host-runtime/src/ring_transport.rs:72`
- `crates/host-runtime/src/config.rs:209`
- `crates/host-runtime/src/ring_transport.rs:1170`

Witness status: yes - `crates/shm-transport/tests/profile.rs:261` checks the charge equals the created object size; `crates/shm-transport/tests/profile.rs:131` and `process_limits_reject_counts_above_the_resident_byte_ceiling` in crates/host-runtime/src/ring_transport.rs. On the host side, `HostLimits::checked_aggregate` (`crates/host-runtime/src/config.rs:209`) states transport, resident, and terminal ceilings as distinct checked quantities and `crates/host-runtime/src/config.rs:615` refuses an unstatable total; `host.status` exposes the aggregate (`crates/host-runtime/src/connection.rs:643`).

## Failure scenario

An under-charged connection oversubscribes the process ceiling.

## Timing windows and dependencies

Each limit one below the requested charge.

## What a test must construct

A limit tightened one unit below one connection's charge, per field.

Situation markers that must fire independently of the safety check:

- `admission.limit_one_below_charge`

Check semantics: `always` - `charges().mapping_bytes == 2 * Ring::object_size()` and `ledger_bytes == 2 * ledger_bytes(geometry)` for the production profile, and every `HostLimits` field is checked in field order.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
