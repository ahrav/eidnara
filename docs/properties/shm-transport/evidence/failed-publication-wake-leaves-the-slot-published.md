# failed-publication-wake-leaves-the-slot-published

## Discovery trigger

Round 19 review of the PR: `failed_publication_wake_leaves_the_slot_published`
is a claim-bearing test with no owning record, and the disposition it pins is
the mirror image of `publish-signal-implies-committed-frame`.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:2364-2375`: `publish_commit`
  writes the descriptor, stores `SLOT_PUBLISHED`, advances `arena_write` and
  `published` through `advance_cursor`, and updates the producer's local
  cursors and `reserved_end`.
- `ring.rs:2376-2379`: the data-doorbell `signal_wake` runs last; on error the
  ring enters quarantine and `commit` returns `ProducerError::Ring(error)`
  with no store undone.
- Test: `failed_publication_wake_leaves_the_slot_published` (`:3973-3996`)
  drops `data_ready.remote`, marks the wake parked so the signal is attempted,
  commits one frame, and asserts `DoorbellFailed`, `is_quarantined()`,
  `SLOT_PUBLISHED`, `reservation_len == 1`, and `published == 1`.

## Failure scenario

A cleanup change on the wake-error path resets the slot to `FREE` or rewinds
`published`. A consumer that already saw the advanced cursor reads a slot whose
state no longer matches, or the next reservation reuses bytes the consumer
holds; the frame is lost or overwritten with no error on the consumer side.

## Timing windows and dependencies

The window is the peer closing its doorbell end between the producer's shared
stores and the signal; a peer that closed earlier fails the pre-commit quarantine
check instead.

## What a test must construct

A closed remote doorbell end and a commit, then assertions on slot state, the
published cursor, and quarantine: present. Missing: the same disposition
observed from an attached consumer handle.

## Investigation log

### Q: Is any shared store undone on the wake-failure path?

- Sources examined: `ring.rs:2364-2380`, `signal_wake`.
- Findings: the error arm calls `enter_quarantine` and returns; no store is
  reverted.
- Missing evidence: none.
- Conclusion: the guarantee holds by construction and the test pins it.
