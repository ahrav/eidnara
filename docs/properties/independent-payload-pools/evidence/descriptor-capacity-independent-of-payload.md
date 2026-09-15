# descriptor-capacity-independent-of-payload

## Discovery trigger

Acknowledging a descriptor makes its queue slot reusable by the producer without the payload having returned and without the request having completed (R1). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1129`
- `crates/shm-transport/src/backend/ring.rs:1356`
- `crates/shm-transport/src/backend/retained.rs:625`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2179` publishes past a one-slot ordinary depth after the consumer acknowledges while still holding the payload; `crates/shm-transport/tests/ring.rs:452` does the same across processes.

## Failure scenario

Retained payloads would stop publication regardless of unused blocks, the FIFO coupling the replacement removes.

## Timing windows and dependencies

None; the property is structural. The interesting window is a producer parked on descriptor exhaustion when the acknowledgement arrives.

## What a test must construct

Ordinary descriptor headroom exhausted with every published payload still held by its lease.

Situation markers that must fire independently of the safety check:

- `pool.ordinary_descriptors_exhausted`
- `pool.payload_held_across_consumption`

Check semantics: `always` - immediately after `try_receive` returns a lease, the producer's `descriptors_outstanding` is one lower than before and a reservation that was `Exhausted` on descriptor headroom alone now succeeds, while `outstanding_returns` still counts the held lease.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
