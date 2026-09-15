# single-replacement-surface

## Discovery trigger

One transport, one layout reader, one receive representation; no FIFO allocator, slot-coupled release, setup decoder, compatibility wrapper, selection flag, or legacy fixture remains (R13). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/arena.rs` and `backend/sample.rs` deleted
- `crates/shm-transport/src/harness.rs:31`

Witness status: yes - `rg` over the tree finds no `SpanPlan`, `SamplePrefix`, `MADV_REMOVE`, `ReleaseSink`, `RingGrant`, `host-test-ring-v1`, or `arena_bytes` in production sources; `crates/shm-transport/tests/contract.rs:167` pins the identifiers.

## Failure scenario

A second transport or reader is an untested path a peer can select.

## Timing windows and dependencies

None.

## What a test must construct

A tree-wide search after every PR.

Situation markers that must fire independently of the safety check:

- `gate.sole_surface_search_run`

Check semantics: `always` - the search list above returns nothing outside `docs/properties` history and this catalog.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
