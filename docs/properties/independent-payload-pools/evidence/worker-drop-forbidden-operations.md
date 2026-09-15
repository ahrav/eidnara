# worker-drop-forbidden-operations

## Discovery trigger

The final drop of a lease performs no `Ring` call, allocates no completion node, waits for no slot, makes no N-API call, and mutates no free list (KTD3). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/lease.rs:377`
- `crates/shm-transport/src/backend/retained.rs:590`
- `crates/shm-transport/src/backend/ring.rs:988`

Witness status: yes - `crates/shm-transport/src/lease.rs:640` and `crates/shm-transport/src/backend/ring.rs:2856` assert the five observers stay unreached through saturated drops on worker threads.

## Failure scenario

A drop that reached the ring or a free list would race the endpoint thread or need it alive, re-coupling lease lifetime to the endpoint.

## Timing windows and dependencies

Drops under class exhaustion, after endpoint exit, and from several worker threads at once.

## What a test must construct

Leases dropped on worker threads while the producer is `Exhausted`; leases dropped after both endpoint handles are gone.

Situation markers that must fire independently of the safety check:

- `lease.drop_during_exhaustion`
- `lease.drop_after_endpoint_exit`

Check semantics: `unreachable` - the five observer code points in `lease::observers` (`ring_call`, `node_allocation`, `slot_wait`, `napi_call`, `free_list_mutation`) are never entered while `in_final_drop` is set; `unreachable` because each is a specific code location that must not execute.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
