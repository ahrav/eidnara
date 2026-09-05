# release-leaves-the-consumer-parked-marker-intact

## Discovery trigger

Round 21 review of the PR: `release_leaves_the_consumers_data_wait_armed_for_the_next_publish`
pins a lost-wake boundary that no record owned.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1528-1533`: `Ring::release` wraps
  `release_inner` and quarantines on error (`:1532-1533`); on the success path
  `release_inner` signals the capacity doorbell (`:1598-1599`) and does not
  touch the data wake epoch, as its doc comment states (`:1518-1523`). On the
  failure path `enter_quarantine` (`:1915-1922`) rings both epochs and so
  clears `parked` by design; that case is owned by
  `quarantine-wakes-a-parked-waiter`.
- `ring.rs:2026-2036`: `signal_wake` bumps the generation and sends a doorbell
  byte only if it swaps a non-zero `parked` (`:2032-2033`), so a cleared marker
  means no signal.
- Test: `release_leaves_the_consumers_data_wait_armed_for_the_next_publish`
  (`:3737-3763`) receives one frame, arms the data wait while holding that lease,
  releases it, asserts `parked` is still non-zero, publishes, and asserts a
  doorbell token is observable through `data_ready.wait_until` within 5 s; no
  thread is blocked, so the wake is asserted as a token left for a later waiter.

## Failure scenario

A release path resets the data wake epoch alongside the capacity one. The
consumer releases its last frame and parks; the producer's next commit finds
`parked == 0`, skips the doorbell, and the consumer sleeps until an unrelated
event; on an idle channel it never wakes.

## Timing windows and dependencies

The window is between `arm_data_wait` and the next `commit`; the test drives
it sequentially on one thread.

## What a test must construct

An armed consumer, a successful release, and a publication in order, asserting
the marker directly and the wake as an observable token: present. Missing: a
waiter actually blocked in `wait_for_data` when the post-release publish lands,
and the release and the publication on different threads.

## Investigation log

### Q: Does any release path touch the data wake?

- Sources examined: `release_inner`, `enter_quarantine`, `signal_wake`.
- Findings: only `enter_quarantine` signals both epochs, and it does so through
  `signal_wake`, which swaps the marker as part of a real wake rather than
  clearing it without one.
- Missing evidence: none.
- Conclusion: the guarantee holds by construction and the test pins it.
