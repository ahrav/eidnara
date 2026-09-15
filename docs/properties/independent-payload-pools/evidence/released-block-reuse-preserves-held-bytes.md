# released-block-reuse-preserves-held-bytes

## Discovery trigger

Releasing payload B permits reuse of B's block while an older payload A remains byte-identical and live (R2); B's reuse never touches A's block. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/pool.rs:285`
- `crates/shm-transport/src/backend/ring.rs:1053`
- `crates/shm-transport/src/lease.rs:308`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2289` holds A for `2*depth+1` reuses of B; `crates/shm-transport/tests/ring.rs:347` repeats it across a process boundary with returns from a worker thread.

## Failure scenario

Reuse that overlapped a live block would corrupt bytes a reader is decoding.

## Timing windows and dependencies

None structural; the enabling state is reuse of a block while another block's lease is live across more than one descriptor lap.

## What a test must construct

At least `2 * descriptor_depth + 1` publications after A while A is held; B's block id repeats.

Situation markers that must fire independently of the safety check:

- `pool.reuse_beyond_descriptor_lap`
- `pool.older_lease_live_during_reuse`

Check semantics: `always` - after each B cycle, `A.to_vec()` equals the bytes captured at A's receive and no B reservation returned A's block id.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
