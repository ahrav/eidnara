# completion-cell-final-owner-once

## Discovery trigger

Each block has one completion cell; the final lease owner publishes its captured generation exactly once with Release and never lowers a newer value (KTD3). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/lease.rs:346`
- `crates/shm-transport/src/backend/retained.rs:584`
- `crates/shm-transport/src/backend/retained.rs:352`

Witness status: yes - `crates/shm-transport/src/lease.rs:544`, `crates/shm-transport/src/lease.rs:573`, and `crates/shm-transport/src/lease.rs:588` run under Miri.

## Failure scenario

A double publication or a lowered cell could free a block twice or hide a real return.

## Timing windows and dependencies

Explicit release followed by drop; a late stale return after reuse; a return from another thread.

## What a test must construct

Explicit `release` then drop of the same lease; a lease dropped on a non-receiving thread; a stale lease returning after its block was republished.

Situation markers that must fire independently of the safety check:

- `lease.explicit_release_then_drop`
- `lease.dropped_on_worker_thread`

Check semantics: `always` - after `release` or drop, the cell holds `max(previous, generation)`, `returned` is set before publication, and a second `release` is `DuplicateRelease` with no second publication.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
