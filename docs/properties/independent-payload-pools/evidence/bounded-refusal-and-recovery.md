# bounded-refusal-and-recovery

## Discovery trigger

Every refusal is bounded and named by resource (class, descriptor headroom, admission field), and recovery follows the resource's own release within one reservation attempt. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1044`
- `crates/shm-transport/src/profile.rs:416`
- `crates/host-runtime/src/ring_transport.rs:279`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2356` and `crates/shm-transport/tests/profile.rs:107`; `crates/host-runtime/src/ring_transport.rs:3435` shows the host names the exhausted resource in `exhaustion.by_resource`, charges nothing, and admits again after release; `crates/host-runtime/src/ring_transport.rs:3253` bounds a stalled peer by the frame deadline; `crates/host-runtime/src/client.rs:7727` shows the client's parked write expiring alone at its operation deadline and retiring the bridge at the frame deadline with nothing published, and `crates/host-runtime/src/client.rs:7086` covers the client budget's exact-fit, one-over, and overflow cases.

## Failure scenario

An unbounded or unrecoverable refusal is a hang the peer cannot diagnose.

## Timing windows and dependencies

Exhaustion of each resource in isolation.

## What a test must construct

One class empty; descriptor headroom full; one admission field at its limit.

Situation markers that must fire independently of the safety check:

- `pool.single_resource_exhausted`
- `admission.field_at_limit`

Check semantics: `always` - after the exhausting resource is released, the next `try_reserve_in` or `admit` succeeds; refusals charge nothing.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
