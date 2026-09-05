# backpressure-converges-in-a-bounded-reclaim-window

## Citation refresh, 2026-08-31 (eventfd rewrite)

PR #131 (merge `5d638e3e8`) replaced the polling wake mechanism with sparse
eventfd doorbells. `reserve_until` no longer spins or sleeps on a poll quantum;
it parks on the `capacity_ready` doorbell. The `HotPinnedPoll`/`ColdParkWake`
modes are gone, `POLL_INTERVAL` survives only in
`crates/host-runtime/tests/support/process_resources.rs:75`, and every line below was
re-verified against HEAD.

## Discovery trigger

Every existing liveness record in this part concerns a fault: a crashed producer, a
stale cursor, a discarded release. None states that the transport makes progress when
nothing is wrong. `reserve_until` is the only place in the transport that converts
"no capacity right now" into "wait and try again", and it retries exactly one error
variant. That makes it the one function whose correctness is the difference between a
transport that applies backpressure and a transport that stalls, and nothing states
what it must achieve.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1345-1390` `reserve_until`. The loop
  retries only `Err(ProducerError::Exhausted)` and only while
  `Instant::now() < deadline` (`:1353`); a sustained `Exhausted` becomes
  `ProducerError::Deadline` (`:1354`, `:1362`, `:1373`, `:1383-1385`);
  every other outcome, success or error, returns immediately. Between attempts
  there is no spin and no sleep: the producer stores a generation-bound park epoch
  on the capacity wake page (`:1357-1359`), re-runs `try_reserve` after parking
  (`:1360`), rechecks the generation (`:1365`), drains the doorbell and re-runs
  `try_reserve` again (`:1368`, `:1371`), rechecks the generation again (`:1376`),
  and only then blocks in `capacity_ready.wait_until(deadline)` (`:1379-1382`), a
  deadline-bounded `poll(2)` on the eventfd (`Doorbell::wait_until`, `:818`).
- The wake edge: `release` signals `capacity_ready` (`:1598-1599`) through
  `signal_wake` (`:2026-2037`), which increments the wake generation and writes
  the eventfd only when a waiter's park flag was set (`:2033`). Delivery is
  sparse by design: an unparked producer gets no write, and the arm/recheck
  protocol above is what closes the race where the release lands between the
  producer's check and its park.
- Three distinct conditions all surface as `Exhausted`, so all three are retried:
  descriptor depth full, `outstanding >= descriptor_depth` (`:1293-1295`); a lost
  `SLOT_FREE → SLOT_PRODUCER_RESERVED` compare-exchange (`:1302-1311`); and arena
  exhaustion from `SpanPlan::reserve`, which rolls the slot back to `SLOT_FREE`
  before returning (`:1312-1317`). `ArenaError::Exhausted` is produced when
  `len > capacity - used` with `used = write - reclaimed`
  (`crates/shm-transport/src/arena.rs:98-106`).
- `ring.rs:1281` — the mechanism that makes retrying useful:
  `try_reserve` calls `self.reclaim_completed()` before reading any cursor. This is
  the **only** call site of `reclaim_completed` in the repository, confirmed by search.
  Reclamation is therefore producer-driven and lazy: a retry is not a passive wait, it
  is the act that recovers capacity.
- `ring.rs:2070-2151` `reclaim_completed` drains the whole contiguous completed prefix
  in one call. The loop advances while `completion_sequence == next`
  (`:2086-2092`) and breaks at the first gap (`:2090-2092`), advancing
  `arena_reclaimed` (`:2143`) and `completed` (`:2144`) after validating the
  run. One call is enough; a second adds nothing unless a new completion landed.
- `ring.rs:1591-1593` — the receiver end of the edge. `release` stores
  `completion_sequence` with `Release` and decrements `active_leases`. The producer's
  matching `Acquire` load is `ring.rs:2090`.
- Existing check, partial: `two_process_zero_copy_exchange_uses_authenticated_grant`
  (`crates/shm-transport/tests/ring.rs:489-543`). The parent fills the arena with
  one `MAX_FRAME_BYTES` frame, then calls `reserve_until(1, .., now + 5s)` while the
  child holds the lease and sleeps 50 ms (`:528-534`, child at `:575`). The
  `.unwrap()` at `:533` is a genuine convergence assertion, and
  `assert!(waiting_since.elapsed() >= Duration::from_millis(25))` at `:583` (source tree; not at HEAD) keeps it
  from passing vacuously. What it does not do: exercise descriptor-depth exhaustion,
  bound convergence any tighter than five seconds, or assert that capacity returned in
  full.
- Existing check, negative direction:
  `retained_oldest_lease_enforces_fifo_reclamation_and_release_validation`
  (`tests/ring.rs:151-155`) asserts `reserve_until(.., Instant::now())` returns
  `Deadline`. That pins the give-up path, not the converge path.

## Failure scenario

1. A producer offers a frame while the arena is full or the descriptor ring is at
   depth. `try_reserve` returns `Exhausted` and `reserve_until` parks a wake epoch
   and, after its rechecks, blocks on the `capacity_ready` doorbell.
2. The receiver drains normally: it acquires the oldest frame, copies it, and
   releases the lease, storing `completion_sequence` (`:1591`) and signalling
   `capacity_ready` (`:1598-1599`).
3. A defect anywhere in the recovery chain — `reclaim_completed` not called from
   the retry path, the loop advancing at most one sequence per call, the `Acquire`
   load at `:2090` weakened, `reserve_until` misclassifying the retryable variant,
   or a wake defect: `release` skipping the doorbell write while the producer's
   park flag is set, or the producer parking without re-running `try_reserve`
   first — leaves the producer asleep or `completed`/`arena_reclaimed` behind
   their true values. The wake-defect family is new with the eventfd mechanism;
   the polling design re-evaluated every 50 microseconds regardless.
4. `reserve_until` keeps returning to the doorbell until the deadline and reports
   `ProducerError::Deadline`. In the host that is an outbound publish failure:
   `publish_one` returns `Err`, the endpoint cancels and returns
   (`crates/host-runtime/src/ring_transport.rs:622-630`), the endpoint thread joins,
   and `admission.release()` runs unconditionally (`:360`) — the pre-refactor
   suspect branch is gone.
5. The operator-visible symptom is a retired generation attributed to a transport
   fault, on a channel where the peer was draining correctly the entire time.

## Timing windows and dependencies

The window opens at the first `Exhausted` and closes at the deadline. Three
quantities bound it and all three must be stated for a test to be refutable.
First, visibility: the producer cannot observe a release until the `Release` store
at `:1591` is visible to the `Acquire` load at `:2090`, which is immediate
in-process and bounded by store propagation across processes. Second, the reclaim
pass: one `try_reserve` recovers the entire contiguous completed prefix, so the
bound after visibility is **one further attempt**, not a number proportional to
the backlog. Third, the wake: a parked producer performs that attempt only after
the `capacity_ready` doorbell fires or its `wait_until` deadline lapses, so
doorbell wake latency replaces the old 50-microsecond poll quantum as the floor on
any asserted bound — and unlike the quantum it is not a code constant, so a test
must choose and record its own inner bound. Dependency on the fault-free window is
strict: the property is stated for a receiver that keeps releasing. Under a
retained lease, non-convergence is the documented FIFO behaviour, which is why
`reclamation-keeps-pace-with-completion` carries the head-of-line case separately.

## What a test must construct

Offered load that actually exhausts capacity, then removal of the pressure, then a
bounded poll. Two arms, because the two exhaustion causes are independent and only
one is covered today. Arm A, arena exhaustion: fill the arena with one maximum
frame, hold its lease, assert `try_reserve` returns `Exhausted`, release, then
assert the **next single** `try_reserve` succeeds — not `reserve_until` with a
generous deadline, because that cannot distinguish one reclaim pass from a
thousand. Arm B, descriptor exhaustion: publish `descriptor_depth` small frames
without receiving, assert `Exhausted`, then receive and release all of them and
assert one `try_reserve` succeeds. Both arms must also assert the negative: with a
deadline set beyond the release, `reserve_until` returns `Ok`, and the elapsed
time is strictly below the deadline, so a `Deadline` return is a failure rather
than a slow pass — under the eventfd mechanism this arm is also the lost-wake
detector, because a parked producer whose `capacity_ready` signal was skipped
converges only at its deadline. Cross-process form: keep the 5-second deadline of
the existing test as an outer safety bound, but assert convergence within an
explicit inner wall-clock bound after the release is observed, chosen and recorded
by the test, and fail if it is exceeded; N poll rounds is no longer a meaningful
unit because the producer does not poll. Enabling situation is already declared in
`fault-map.md` as `shm_arena_wrap_with_live_lease`; arm B needs no new marker
because descriptor saturation without receipt is trivially reachable.

## Investigation log

### Q: Is `reserve_until` convergence guaranteed by construction, or is there a state where retrying cannot help?

- Sources examined: `ring.rs:1267-1340`, `:2039-2046`, `:2075-2151`; `arena.rs:84-123`;
  `crates/host-runtime/src/ring_transport.rs:33`, `:44`, `:447-451`, `:580-606`;
  `tests/ring.rs:123-173`, `:488-578`.
- Findings: the retry set is exactly right for the fault-free case. All three
  exhaustion causes map to the one retried variant, and the slot rollback at `:1315`
  and `:1319` means a failed arena plan does not leak the descriptor slot it had already
  claimed. The compare-exchange loss at `:1302-1311` deserved a check of its own, since a
  losing CAS is normally permanent: `slot_ptr` maps sequence to index `(sequence - 1) %
  descriptor_depth` (`:2043`), so the slot for `published + 1` was last used by
  sequence `published + 1 - depth`, and the depth gate at `:1293` already guarantees
  that sequence is at or below `completed`, hence reclaimed to `SLOT_FREE` at `:2140`.
  In the fault-free case the CAS therefore cannot lose, and the `Exhausted` at `:700` (source tree; not at HEAD)
  is a defensive path. It is the same path a killed producer wedges permanently, which
  is `crashed-producer-does-not-wedge-the-sequence` and is out of scope here.
- Missing evidence: nothing for the mechanism. Untested rather than unknown is the
  cross-process visibility bound; the existing two-process test uses a 5-second
  deadline and a 50 ms sleep, which is three orders of magnitude of slack and so
  measures nothing about latency.
- Conclusion: resolved with answer — convergence holds by construction in the
  fault-free case, and the tight bound is one `try_reserve` after the release becomes
  visible, because `reclaim_completed` drains the entire contiguous prefix per call.
  The property is worth cataloging because that bound is nowhere asserted, and the one
  test that touches convergence uses a deadline loose enough to hide a reclaimer that
  advanced one sequence at a time.

### 2026-08-31: re-derivation against the eventfd doorbell mechanism

- Sources examined: `crates/shm-transport/src/backend/ring.rs:714-842`,
  `:1267-1340`, `:1345-1390`, `:1591-1599`, `:2026-2037`, `:2070-2151`;
  `crates/shm-transport/src/arena.rs:84-123`;
  `crates/host-runtime/src/ring_transport.rs:360`, `:622-630`, `:560-630`;
  `crates/shm-transport/tests/ring.rs:123-173`, `:488-578`.
- Findings: the retry set, the single `reclaim_completed` call site, and the
  one-pass prefix drain all survive PR #131; the pre-refactor citation targets
  moved but the logic is line-for-line recognisable. What changed is the wait
  between retries: `reserve_until` went from a 50-microsecond sleep loop
  (`ColdParkWake`) to a park/recheck/park protocol against the `capacity_ready`
  doorbell, with `release` signalling only a parked waiter. The convergence
  argument therefore gains a step: the release must not only publish
  `completion_sequence`, it must wake the producer, and the arm/recheck ordering
  (`try_reserve` re-run after every park and after every drain) is what makes a
  release that lands mid-arm safe. The old "one poll quantum is the floor" clause
  is void; nothing in the code constant-bounds wake latency.
- Missing evidence: unchanged — the cross-process visibility bound is untested,
  and there is now additionally no measured figure for doorbell wake latency to
  inform the inner bound a test should assert.
- Conclusion: resolved with answer — the guarantee and the one-attempt reclaim
  bound survive; the poll-quantum floor is replaced by doorbell wake latency, the
  `reserve_until` arm doubles as the lost-wake detector, and the inner
  cross-process bound must be a recorded wall-clock choice rather than a count of
  poll rounds.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 34, `:450` now `:818`: `Doorbell::wait_until` runs a deadline-bounded `poll(2)` on this handle's AF_UNIX socketpair end, not on an eventfd.
  - line 37, `:1427` now `:2033`: `signal_wake` increments the wake generation and, when it swaps a nonzero `parked`, sends a one-byte token on the AF_UNIX socketpair doorbell (`ring.rs:2033-2035`, `Doorbell::signal` at `:783-798`).
  - line 43, `:938-943` now `:1302-1311`: A lost slot compare-exchange no longer reports `Exhausted`: it quarantines the ring and returns `ProducerError::Ring(RingError::InvalidSharedState)`, so only descriptor-depth saturation and arena exhaustion surface as `Exhausted`.
  - line 66, `:583`: Nothing in the two-process test keeps the `reserve_until` from passing on a wait that never blocked; its non-vacuity now rests on the reclaimed descriptor and byte counts asserted after the child exits.
  - line 95, `:276` now `:360`: `admission.release()` is not unconditional: the endpoint thread calls `admission.quarantine()` instead when either ring latched quarantine and the peer did not release it (`crates/host-runtime/src/ring_transport.rs:353-361`).
  - line 151, `:700` now `:1302-1311`: A losing compare-exchange now quarantines the ring with `RingError::InvalidSharedState` rather than returning `Exhausted`.
  - line 172, `crates/shm-transport/src/backend/ring.rs:384-467` now `crates/shm-transport/src/backend/ring.rs:714-842`: The doorbell is an AF_UNIX `socketpair` with a nonblocking local end, a movable peer end, a bounded token `drain`, and a `poll(2)` wait, not an eventfd.
  - line 175, `crates/host-runtime/src/ring_transport.rs:276` now `crates/host-runtime/src/ring_transport.rs:360`: The release is taken only on the non-quarantined branch; a quarantined ring goes to `admission.quarantine()` (`ring_transport.rs:353-361`).
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 66, `:583` (elapsed-time lower bound on the parked wait): No elapsed-time assertion remains in the test; the post-exchange `conservation()` checks (`crates/shm-transport/tests/ring.rs:540-542`) are the only convergence evidence.
  - line 156, `:700` (an Exhausted return on a lost compare-exchange): The compare-exchange at `ring.rs:1302-1311` has no `Exhausted` exit; its defensive path is a quarantine with `RingError::InvalidSharedState`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
