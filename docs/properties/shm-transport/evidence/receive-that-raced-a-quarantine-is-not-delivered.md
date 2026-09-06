# receive-that-raced-a-quarantine-is-not-delivered

## Discovery trigger

Round 25 review of the PR: the consumer-side post-check in `Ring::try_receive`
has a test and no owning record, while its producer-side twin is owned.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1396-1408`: `try_receive` checks
  quarantine at entry, calls `try_receive_inner` under `quarantine_with`, then
  re-reads `is_quarantined()` and drops the lease with `Err(Quarantined)`
  (`:1402-1407`); the comment at `:1402-1403` states the intent.
- `ring.rs:1431-1438`: the inner compare-exchange that takes the slot into
  `SLOT_RECEIVER_HELD`; the window this record guards lies between it and the
  post-check.
- `ring.rs:2382-2384`: the producer-side twin in `publish_commit`.
- Test: `receive_that_raced_a_quarantine_is_not_reported_as_delivered`
  (`:3705-3716`) sets the shared flag, shows `try_receive_inner` still yields a
  lease, and asserts the public wrapper reports `Err(Quarantined)`.

## Failure scenario

The post-check is removed. A peer quarantines between the consumer's entry
check and its slot take; the consumer receives a lease over a terminal ring,
reads bytes the peer may have abandoned, and its release fails with
`Quarantined` after the frame was treated as delivered.

## Timing windows and dependencies

The window is the inner take itself; the test sets the flag before the public
call and so proves the check, not the race.

## What a test must construct

A quarantine flag set before `try_receive` and an `Err(Quarantined)` result:
present. Missing: the flag set by a second thread between the inner
compare-exchange and the post-check.

## Investigation log

### Q: Does the test reach the post-check or only the entry check?

- Sources examined: `ring.rs:1396-1408`, `:3705-3716`.
- Findings: the flag is set before the public call, so the entry check at
  `:1396-1398` would already refuse; the test reaches the post-check only
  because it first calls `try_receive_inner` directly, which has no entry check
  of its own, to show the inner path would have yielded a lease.
- Missing evidence: a race construction.
- Conclusion: Exercised is partial.
