# payload-identity-authorizes-reuse

## Discovery trigger

A block is reused only when its completion cell holds exactly the generation the producer issued; a stale cell frees nothing, a cell ahead of any issued generation quarantines, and no pointer or peer offset is ever consulted (KTD2). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1171`
- `crates/shm-transport/src/backend/retained.rs:597`
- `crates/shm-transport/src/descriptor.rs:222`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2635` and `crates/shm-transport/src/backend/ring.rs:2540`; `crates/shm-transport/tests/contract.rs:118` covers the pure validator.

## Failure scenario

A stale or forged return could free a block a newer occupant still reads, the early-reuse stop condition.

## Timing windows and dependencies

A stale return landing after the block was reused with a newer generation; a forged cell value above the issued generation.

## What a test must construct

A block reused at least once (generation >= 2) with a late return of the earlier generation; a cell written past the issued generation.

Situation markers that must fire independently of the safety check:

- `pool.block_reused_with_newer_generation`
- `pool.completion_cell_ahead_of_issue`

Check semantics: `always` - in `reclaim_completions`, every block pushed to a free list satisfies `cell == ledger.generations[block]`, and every observed `cell > generation` ends in quarantine before any free-list mutation.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
