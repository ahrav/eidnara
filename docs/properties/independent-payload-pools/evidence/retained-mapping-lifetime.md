# retained-mapping-lifetime

## Discovery trigger

Backing, completion cells, wake handle, and the backing charge stay alive while any lease exists; the mapping unmaps only when the last holder drops; endpoint exit refunds worker count only (KTD5). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/lease.rs:248`
- `crates/shm-transport/src/backend/retained.rs:270`
- `crates/host-runtime/src/ring_transport.rs:427`

Witness status: yes - `crates/shm-transport/src/lease.rs:625` (Miri) and `crates/shm-transport/src/backend/ring.rs:2535`.

## Failure scenario

Unmapping under a reader is a use-after-unmap; refunding its charge early lets admission oversubscribe.

## Timing windows and dependencies

Endpoint close and reconnect with a reader still holding a lease.

## What a test must construct

Both endpoint handles dropped while a lease is live; a late return after the drop.

Situation markers that must fire independently of the safety check:

- `lease.live_after_endpoint_exit`
- `lease.late_return_after_endpoint_exit`

Check semantics: `always` - with a lease live, `Weak::upgrade` on the backing succeeds after both `Ring` handles drop and `to_vec` returns the original bytes; after the lease drops, the upgrade fails.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
