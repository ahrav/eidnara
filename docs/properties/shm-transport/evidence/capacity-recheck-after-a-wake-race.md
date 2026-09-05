# capacity-recheck-after-a-wake-race

## Discovery trigger

Fix commit `a36f6e687` "Readiness races could strand full rings and miss idle
peer death. Recheck capacity, watch setup sockets, and keep callbacks
progressing." Its `ring.rs` hunk inserted the two post-park `try_reserve`
retries into `reserve_until`. Lead only; the loop was re-read at HEAD.

## Evidence trail

- `reserve_until` (`crates/shm-transport/src/backend/ring.rs:1345-1390`)
  parks a generation-bound epoch before blocking: read `generation`
  (`:646`), store `parked = generation + 1` (`:647-648`).
- Between parking and blocking, the loop closes the race window three ways:
  1. re-run `try_reserve` immediately after parking (`:1360-1364`);
  2. recheck the generation (`:1365-1367`), drain the doorbell
     (`:1368-1370`), re-run `try_reserve` again (`:1371-1375`);
  3. recheck the generation once more (`:1376-1378`) and only then block in
     `capacity_ready.wait_until(deadline)` (`:1379-1385`).
- The publisher's half: `release` signals `capacity_ready` through
  `signal_wake` (`:1598-1599`, `:2026-2037`), which increments `generation`
  with SeqCst (`:2032`) and writes the eventfd only when it swapped a nonzero
  `parked` (`:2033-2035`).
- Case analysis for a release concurrent with the park: if the release's
  `generation.fetch_add` precedes the producer's generation read, the freed
  capacity is visible to the first `try_reserve`; if it lands after the read
  but before blocking, either a generation recheck fails (continue, no
  block), or the release's `parked.swap` saw the producer's epoch and wrote
  the eventfd, which `wait_until` observes as `POLLIN`. No interleaving
  leaves the producer blocked while capacity is free — under the SeqCst
  orderings the code claims, which no tool currently validates.
- `arm_data_wait` (`:1187-1220`) is the same protocol for the consumer
  direction, with the same recheck-after-park and recheck-after-drain shape.
  At HEAD: The wake write is a one-byte token sent on the AF_UNIX socketpair doorbell (`Doorbell::signal`, `ring.rs:783-798`), which `wait_until` observes as `POLLIN` on that socket.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

Without the post-park retries: producer sees Exhausted, reads generation G,
parks G+1. Receiver releases, bumps to G+1, swaps `parked` to 0, writes the
eventfd. Producer drains the doorbell as part of stale-token hygiene,
consuming the wake, then blocks until its deadline although the ring has
room. Result: `ProducerError::Deadline` on a ring with free capacity, which
the host reports as a failed publish on a healthy channel — the
strand-a-full-ring symptom the fix commit names.

## Timing windows and dependencies

The vulnerable window is generation-read to `poll` entry, a few dozen
instructions wide, so only a true concurrent releaser (fault class F4) or a
model checker reaches it deliberately. Correctness rests on SeqCst ordering
between `generation` and `parked` on both sides (F5 territory: no loom,
Miri, or TSan run exists). Bounded claim: capacity freed at any point before
the block is consumed within the same loop iteration; capacity freed during
the block terminates the block via the eventfd.

## What a test must construct

A parked producer plus a release racing the arm sequence, repeated enough to
land in the window, with the oracle that `reserve_until` returns success
strictly before its deadline. Today
`two_process_zero_copy_exchange_uses_authenticated_grant`
(`crates/shm-transport/tests/ring.rs:489-543`) blocks a `reserve_until`
behind a child's held lease and converges after the child releases 50 ms
later — it exercises the block-then-wake path but the release always lands
well inside the block, never in the arm window. A loom model is the cheap
oracle, but it is necessarily a hand transcription of `reserve_until` and
`signal_wake` over loom atomics — the protocol's atomics live in an mmapped
shared page loom cannot instrument — kept in sync manually, including the
Release-not-SeqCst parked resets.

## Investigation log

### Q: why two try_reserve retries rather than one?

- Sources examined: `:1360-1375`; fix diff of `a36f6e687`.
- Findings: the first retry catches releases before the park was visible;
  the drain between them can consume a stale token from a wake that predates
  this park epoch, so capacity freed by that earlier wake must be re-checked
  after the drain or it is only represented by a token the producer just
  discarded.
- Missing evidence: none.
- Conclusion: resolved with answer — the drain is why the second retry
  exists.

### Q: are the orderings actually sufficient?

- Sources examined: `:646-648`, `:2032-2035`; generation is SeqCst on both
  sides, but every `parked` reset on the exit paths is a `Release` store
  (`:655`, `:655`, and the other exit arms through `:655`), not SeqCst.
  The mix is harmless on the current reading — a resetting producer is by
  definition not blocked — but a loom model that silently "corrected" it to
  SeqCst would validate different code.
- Findings: the pairing argument above is by hand; no concurrency tool runs
  anywhere in the repository (existing-checks.md, concurrency section).
- Missing evidence: no loom or Miri model of the park/wake protocol exists
  anywhere in the repository.
- Conclusion: unresolved, needs a loom or Miri pass over the wake protocol.
  At HEAD: `parked` is cleared in exactly one place at HEAD, `ParkGuard::drop` (`ring.rs:653-657`), with a `Release` store, so the mixed-ordering observation concerns that single site rather than several exit arms.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 24, `:1427-1429` now `:2033-2035`: The wake write is a one-byte token sent on the AF_UNIX socketpair doorbell (`Doorbell::signal`, `ring.rs:783-798`), which `wait_until` observes as `POLLIN` on that socket.
  - line 89, `:1004` now `:655`: `parked` is cleared in exactly one place at HEAD, `ParkGuard::drop` (`ring.rs:653-657`), with a `Release` store, so the mixed-ordering observation concerns that single site rather than several exit arms.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
