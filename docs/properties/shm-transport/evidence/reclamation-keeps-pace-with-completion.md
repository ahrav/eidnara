# reclamation-keeps-pace-with-completion

## Discovery trigger

Arena reclamation is strict FIFO: one retained lease pins `arena_reclaimed` and blocks
byte reclamation for every sequence behind it, however many have been released. The
catalog already covers the pathological side of that design — a stale cursor, a
peer-chosen advance — but not the ordinary side. Nothing states that when no lease is
retained, reclamation actually catches up, or how far it must catch up per pass. The
existing FIFO test releases the blocking lease and then asks for a single byte, which
one reclaimed sequence out of two satisfies.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:2070-2151` `reclaim_completed`. It reads
  `completed` (`:2077`), then loops: for `next = completed + 1` it loads
  `completion_sequence` with `Acquire` (`:2090`) and **breaks at the first gap**
  (`:2091-2092`). That break is the head-of-line mechanism. When the sequence is
  contiguous it requires `SLOT_RELEASE_PENDING` (`:2093-2095`), revalidates the
  descriptor (`:2096-2101`), requires `allocation_start == arena_reclaimed`
  (`:2112-2114`), then advances `arena_reclaimed` (`:2143`), clears
  `reservation_len` and `completion_sequence`, frees the slot, and stores `completed`
  (`:2138-2144`). The loop continues, so **one call drains the entire contiguous
  completed prefix**, not one sequence.
- `ring.rs:1281`: in the source tree this record was written against, the only call
  site of `reclaim_completed` in the repository, confirmed by search: the first
  statement of `try_reserve`. Reclamation is producer-driven and
  lazy. A receiver that releases everything while the producer never reserves or trims
  leaves every byte and every slot charged indefinitely, and that is by design rather
  than a defect. It fixes the shape of the bound: the window is counted in producer
  reserve attempts, not in wall-clock time.
- The two capacities reclamation feeds. Descriptors: `try_reserve` computes
  `outstanding = published - completed` and refuses at `outstanding >=
  descriptor_depth` (`:1290-1295`). Bytes: `SpanPlan::reserve` computes `used = write -
  reclaimed` and returns `ArenaError::Exhausted` when `len > capacity - used`
  (`crates/shm-transport/src/arena.rs:98-106`). Both cursors advance only in
  `reclaim_completed`, at `:2144` and `:2143`.
- `ring.rs:1590-1591` — the receiver's half of the edge: `completion_sequence` stored with
  `Release` after the `SLOT_RECEIVER_LEASED → SLOT_RELEASE_PENDING` compare-exchange at
  `:1575-1580`. `Release` at `:1591` pairs with `Acquire` at `:2090`.
- The observable healthy state: `conservation()` (`:1607-1744`) counts `SLOT_FREE` into
  `descriptors.free` (`:1685`) and derives `bytes.free = arena_bytes - charged`
  (`:1739-1743`). Full recovery is `descriptors.free == descriptor_depth` and
  `bytes.free == arena_bytes`. Caveat carried from
  `reservation-charge-visible-with-non-free-state`: `bytes.free` is derived, so
  `ArenaCounts::conserves` is arithmetically self-satisfying and only
  `descriptors.free` plus a successful full-size reserve are independent evidence.
- Existing check, and the exact gap:
  `retained_oldest_lease_enforces_fifo_reclamation_and_release_validation`
  (`crates/shm-transport/tests/ring.rs:124-174`). It publishes 40 MiB at sequence 1
  and holds its lease (`:129-137`), publishes the remaining 24 MiB at sequence 2 and
  releases it immediately (`:139-146`), and asserts the head-of-line consequence:
  `try_reserve(1)` is `Exhausted` (`:148-151`), `bytes.free == 0` (`:160`),
  `receiver_leased == 1` and `release_pending == 1` (`:158-159`). That half is good. Then
  it releases the retained lease (`:162`) and asserts `try_reserve(1)` succeeds
  (`:163`). With `arena_bytes == MAX_FRAME_BYTES == 64 MiB` fully charged, a
  reclaimer that advanced only sequence 1 would leave 40 MiB free — and a one-byte
  request succeeds against 40 MiB exactly as it succeeds against 64 MiB. The test cannot
  see the difference.
- `boundary_round_trips_include_wrap_and_exact_maximum` (`tests/ring.rs:41-121`) does
  assert full recovery at `:116-120`, but only after a strictly serial
  publish-receive-release cycle where the prefix is never non-contiguous, so it never
  exercises catch-up across more than one sequence.
  At HEAD: The test is now named `retained_oldest_lease_enforces_fifo_reclamation`; the release-validation arm named in the old title is gone, and the recovery arm additionally asserts `resident_arena_pages() == 0` (`:164-168`) while still asking for only one byte.
  At HEAD: It is no longer the only call site: `Ring::trim` (`:2247`) runs a reclaim pass at `:2256` precisely so an idle ring can return capacity without a reserve attempt, so the bound is producer reserve attempts or explicit trims.
  At HEAD: The advance no longer happens per sequence inside the loop: the loop only accumulates `run_len` (`:2115-2117`), and `arena_reclaimed` moves once after the loop through `advance_cursor` at `:2143`, which strengthens the one-pass claim this record makes.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. A producer publishes several frames. The receiver acquires them and releases all but
   the oldest, so `completion_sequence` is set for the newer sequences while sequence
   `k` stays `SLOT_RECEIVER_LEASED`.
2. `reclaim_completed` breaks at `k` (`:2090-2091`). `arena_reclaimed` and `completed`
   stay pinned. `try_reserve` reports `Exhausted` from either the depth gate or the
   arena gate. This is correct, documented behaviour.
3. The receiver releases lease `k`. The prefix is now contiguous from `completed + 1`
   through the newest released sequence.
4. A defect that makes the loop advance one sequence per call — an early `break`, a
   `return Ok(())` inside the body, or the `completed` reload moved outside the loop —
   leaves the remaining sequences charged after the first reserve.
5. Consequence: capacity returns at one sequence per producer reserve attempt instead of
   all at once. Under `reserve_until` the producer still converges, because each retry
   is another pass, so the defect is invisible to any test with a loose deadline. What
   breaks is the size class: a producer asking for a large frame is refused while the
   arena is mostly reclaimable, and reports `Deadline` on a healthy channel. In the host
   that is a publish failure that cancels the generation
   (`crates/host-runtime/src/ring_transport.rs:622-630`). The
   accounting stays self-consistent throughout, so nothing else signals it.

## Timing windows and dependencies

No race window; both cursors are producer-owned and written only in
`reclaim_completed`. The bound has two parts. Visibility: the receiver's `Release` store
at `:1591` must be visible to the `Acquire` load at `:2090`, immediate in-process and
bounded by store propagation across processes. Progress: after visibility, **one**
`try_reserve`, or one explicit `Ring::trim` (`:2247`, whose first step is the same
`reclaim_completed` pass at `:2256`), must reclaim the entire contiguous prefix,
because the loop is written to do exactly that. So the fault-free window is one
producer reserve attempt or one producer-side trim, and any need for a second is the
defect. The producer-driven dependency is strict and is the reason the bound cannot be
phrased in wall-clock terms: with no producer reserve or trim, elapsed time buys
nothing; `trim` exists so an idle producer can still return capacity, but something
on the producer side must call it. The head-of-line precondition is the situation
`shm_arena_wrap_with_live_lease`, already declared in `fault-map.md`, and it is what
makes the non-contiguous prefix exist in the first place.

## What a test must construct

A non-contiguous completed prefix of length at least two behind a retained lease, then
release, then a single-pass full-recovery assertion. Concretely, against the contract
profile: publish and acquire sequence 1 and hold it; publish, acquire, and release
sequences 2 and 3; assert `descriptors.release_pending == 2` and
`descriptors.receiver_leased == 1` so the non-contiguous shape is witnessed rather than
assumed; release sequence 1; then perform exactly **one** `try_reserve` and assert
`descriptors.free == descriptor_depth` and `bytes.free == arena_bytes` after it. A
second arm should repeat the shape with a single `Ring::trim` in place of the
`try_reserve`, since that is the idle-ring reclamation path. The
size of the request matters: ask for a frame that only fits if every sequence was
reclaimed, so the assertion has an independent witness beyond the derived `bytes.free`.
That single change closes the gap in the existing test, which asks for one byte. Add the
head-of-line arm as the negative control in the same test: before releasing sequence 1,
assert a full-size request is `Exhausted`, so the recovery assertion cannot pass by the
capacity having been available all along. Do not use `reserve_until` for either arm —
its retry loop performs additional reclaim passes and destroys the one-pass bound.

## Investigation log

### Q: Does one `reclaim_completed` call recover the whole contiguous prefix, and is that anywhere asserted?

- Sources examined: `ring.rs:1267-1340`, `:1345-1390`, `:1528-1600`, `:1607-1744`,
  `:2070-2151`; `arena.rs:84-123`, `:206-223`; `tests/ring.rs:41-121`, `:124-174`;
  `crates/host-runtime/src/ring_transport.rs:622-630`.
- Findings: yes to the first, no to the second. The loop structure at
  `backend/ring.rs:2086-2119` is
  unambiguous — the only exits are the gap `break`, an error, or exhausting the prefix —
  so one call drains everything contiguous. Nothing asserts it. The FIFO test asserts
  the blocking half well and the recovery half with a one-byte request against a 64 MiB
  arena where 40 MiB would also pass. I also checked whether `reserve_until` masks the
  defect and it does: each retry is another pass, so convergence still happens and only
  the large-frame case fails. That is why the check must be a single `try_reserve` and
  must request a size that needs the full arena.
- Missing evidence: nothing for the mechanism. Untested rather than unknown is the
  cross-process visibility bound, shared with
  `backpressure-converges-in-a-bounded-reclaim-window`; the one two-process test uses a
  50 ms sleep against a 5-second deadline and measures no latency.
- Conclusion: resolved with answer — the healthy-case liveness statement is "one
  producer reserve attempt restores the entire contiguous prefix", the distinguishing
  assertion against head-of-line blocking is a full-size request rather than a one-byte
  request, and the bound must be counted in producer attempts because `try_reserve` and
  the explicit `Ring::trim` are the only drivers of reclamation (in the source tree this
  record was written against, `try_reserve` was the sole one).

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 21, `:1138-1145` now `:2143`: The advance no longer happens per sequence inside the loop: the loop only accumulates `run_len` (`:2115-2117`), and `arena_reclaimed` moves once after the loop through `advance_cursor` at `:2143`, which strengthens the one-pass claim this record makes.
  - line 25, `ring.rs:675` now `ring.rs:1281`: It is no longer the only call site: `Ring::trim` (`:2247`) runs a reclaim pass at `:2256` precisely so an idle ring can return capacity without a reserve attempt, so the bound is producer reserve attempts or explicit trims.
  - line 49, `crates/shm-transport/tests/ring.rs:138-209` now `crates/shm-transport/tests/ring.rs:124-174`: The test is now named `retained_oldest_lease_enforces_fifo_reclamation`; the release-validation arm named in the old title is gone, and the recovery arm additionally asserts `resident_arena_pages() == 0` (`:164-168`) while still asking for only one byte.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
