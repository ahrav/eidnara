# wake-failure-preserves-published-ownership

## Discovery trigger

A doorbell failure after publication or consumption quarantines the handle that rang it, or surfaces as `WakeFailed` to a lease's explicit `release` caller, but never rolls back the published descriptor, the consumption, or the completion; `WouldBlock` is success (KTD3). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1343`
- `crates/shm-transport/src/backend/retained.rs:628`
- `crates/shm-transport/src/backend/retained.rs:644`

Witness status: yes - publish side: `wake_failure_after_publication_quarantines_but_leaves_the_frame_published`; consumption side: `a_failed_consumption_wake_quarantines_the_consumer_and_returns_the_block`; return side: `a_failed_return_wake_reports_wake_failed_and_keeps_the_completion`, which arms `parked`, closes the producer's doorbell end, asserts `WakeFailed` from `release`, and reads the completion cell and return flag back. All three are in `crates/shm-transport/src/backend/ring.rs`.

## Failure scenario

Rolling back a publication the peer may already hold would reuse bytes a reader is decoding.

## Timing windows and dependencies

Peer doorbell end closed before the wake; a full socket buffer (`WouldBlock`).

## What a test must construct

`parked` set with the peer's doorbell end closed so `send` fails with `EPIPE`.

Situation markers that must fire independently of the safety check:

- `pool.wake_send_failed_after_publication`
- `pool.wake_would_block_token_pending`

Check semantics: `always` - after a failed wake, `published` still holds the new sequence or the completion cell still holds the generation, and no free-list mutation followed the failure.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
