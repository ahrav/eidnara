# reservation-charge-visible-with-non-free-state

## Discovery trigger

A comment-versus-code lens over the `unsafe` blocks. The SAFETY comment above the
`reservation_len` load in `conservation()` asserts an ordering that `try_reserve`
does not establish. The comment is the discovery: it states the property, so it
is a claim the code is supposed to satisfy.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1267-1340` is `try_reserve`. The
  relevant order is: the quarantine gate at `:1275`; `reclaim_completed()` at
  `:1281`; the depth check at `:1293-1295`; the sequence derivation at `:1296-1298`;
  the `SLOT_FREE -> SLOT_PRODUCER_RESERVED` compare-exchange at `:1302-1311`; the
  arena planning at `:1312-1324`; and only then the `reservation_len` store at
  `:1325-1326`. **Correction:** the catalog says the plan "returns early at `:710`
  and `:715`", which are now `:1315` and `:1319`. Those lines are the rollback
  `(*slot).state.store(SLOT_FREE, Ordering::Release)` calls; the early returns
  are at `:1316` (`ProducerError::Exhausted`) and `:1322`
  (`ProducerError::Arena(error)`). The rollback ordering matters to the analysis,
  so the distinction is worth keeping.
- `ring.rs:1682-1683` is the contradiction, quoted exactly:
  At HEAD: The quoted lines no longer exist: the comment reads "The reservation length is assigned before any non-free state becomes visible" and the load is safe code (`let len = slot.reservation_len.load(Ordering::Relaxed);`), so the SAFETY wording and the unsafe block are gone while the false ordering claim survives.

  ```rust
  // SAFETY: reservation length is atomic and assigned before non-free state is observed.
  let len = unsafe { (*slot).reservation_len.load(Ordering::Relaxed) };
  ```

  The first half is true: the field is `AtomicU64` (`ring.rs:150`). The second
  half is false: the CAS at `:1302-1311` makes the state non-free, and the store at
  `:1325-1326` happens afterwards.
- `ring.rs:1681` loads `state` with `Ordering::Acquire` immediately before, and the
  `reservation_len` load at `:1683` is `Ordering::Relaxed`. So even the intended
  ordering would not be established by these two loads on their own.
- A grep for `reservation_len` in the file returns six sites: the field
  declaration at `:150`, the store in `try_reserve` at `:1326`, the load in
  `conservation` at `:1683`, the zeroing in `reclaim_completed` at `:2138`, the
  zeroing in `abort_reservation` at `:2279`, and the initialization at `:2791`.
  **`:1683` is the only reader.** So the field exists solely to feed the
  conservation snapshot.
- The rollback path narrows the window in one direction and not the other. On
  arena failure the state returns to `SLOT_FREE` at `:1315` or `:1319` before the
  early return, so an observer never sees a stranded `PRODUCER_RESERVED` slot from
  a failed plan. The window that remains is the successful path between the CAS
  at `:1302-1311` and the store at `:1325-1326`, during which the slot reads
  `PRODUCER_RESERVED` with the previous occupant's `reservation_len`, which
  `reclaim_completed` and `abort_reservation` both zero, so in practice the stale
  value read is `0`.
- `ring.rs:1739-1743` computes `bytes.free = self.grant.arena_bytes.checked_sub(
  charged)`, where `charged` is the running sum of the per-state buckets built at
  `:1684-1737`.
- `crates/shm-transport/src/arena.rs:208-222` is `ArenaCounts::conserves`. It
  sums `free`, `producer_reserved`, `published`, `receiver_held`,
  `receiver_leased`, `release_pending`, `pad`, and `quarantined` and compares to
  capacity. Because `free` is defined as `arena_bytes - charged` and `charged` is
  the sum of the other nonzero buckets, the total is identically `arena_bytes`.
  The catalog's claim that `conserves` is arithmetically self-satisfying is
  confirmed by direct read: it cannot detect an under-counted bucket.
- Existing checks: `crates/shm-transport/tests/ring.rs:116-120`, `:157-160`,
  and `:193-197` all call `conservation()` single-threaded between operations, so
  no reservation is ever open when they read. The catalog's ranges are accurate
  to within a line.
At HEAD: It is no longer the only reader: `validate_idle_window` also loads `reservation_len` with Acquire at `:1837` and compares it against the descriptor's `allocation_len` (`:1851`), and the grep now returns ten non-test sites rather than six.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. The producer calls `try_reserve(bound, header)`.
2. The compare-exchange at `ring.rs:1302-1311` succeeds, so slot N reads
   `SLOT_PRODUCER_RESERVED`.
3. Before `:1325-1326` executes, an observer in another process calls
   `conservation()`. It loads `state` at `:1681` and sees
   `SLOT_PRODUCER_RESERVED`, then loads `reservation_len` at `:1683` and gets the
   zeroed residual value.
4. `descriptors.producer_reserved` is incremented at `:1687` while
   `bytes.producer_reserved` gains `0` at `:1688-1691`, and `charged` also gains
   `0`.
5. `bytes.free` at `:1739-1743` is therefore computed as the full arena, so the
   snapshot reports a slot reserved and zero bytes charged for it, and
   `ArenaCounts::conserves` still returns true because the two errors cancel by
   construction.

The consequence is byte-accounting inaccuracy in a cross-process snapshot, not
memory unsafety. No pointer, length, or span is derived from `reservation_len`;
`reclaim_completed` derives its advance from the descriptor's `allocation_len`
instead (`ring.rs:2115-2117`), which is the subject of a separate record.

## Timing windows and dependencies

The window is the instruction interval between the CAS at `:1302-1311` and the
store at `:1325-1326`, containing only two atomic loads at `:703` (source tree; not at HEAD) and `:705` (source tree; not at HEAD) and
the `SpanPlan::reserve` call at `:1312`. It is short but not bounded by anything,
and on a failed plan the state is rolled back before the observer could
misattribute it, so the only observable case is the success path. There is a
second, wider dependency that changes this property's priority sharply: at this
commit `conservation()` has no production caller. Its only non-test caller is
`Ring::probe()` at `ring.rs:1887`, and the only production `probe` implementation,
`ShmRecoveryBackend::probe` (`crates/host-runtime/src/shm_provider.rs:143-147` at
`9c1eb4d1`), returned `true` unconditionally with the comment "No shared state
outlives the endpoint thread, so isolation alone proves the provider side is
clean" and never touched a `Ring`. `ed487e11` deleted that implementation with
`shm_provider.rs` and `provider_recovery.rs`, and nothing in the tree replaces it,
so the observer this property protects still does not exist.
At HEAD: The same walk now has a production caller through a different entry point: `Ring::attach` calls `conservation_inner(true)` at `:1148`, so a peer-broken mapping is refused at attach time even though `conservation()` itself stays probe-only.
At HEAD: The window no longer contains any atomic load: `verified_producer_cursors` (`:1287-1289`) reads `arena_write` and `arena_reclaimed` before the exchange, so only the `SpanPlan::reserve` call sits between the exchange and the store.

## What a test must construct

Two producer and observer roles that are genuinely concurrent, which the current
harness cannot do: the only cross-process test,
`two_process_zero_copy_exchange_uses_authenticated_grant`
(`crates/shm-transport/tests/ring.rs:489`), is lockstep with a sleep. The
concrete construction is a deterministic pause between `ring.rs:1311` and `:1325`,
reached by a failpoint rather than by timing, with a second process calling
`conservation()` while the pause is held. The oracle must not be
`ArenaCounts::conserves`, which passes by construction. It must be the per-slot
cross-check the catalog names: for every slot whose state is not `SLOT_FREE`,
assert `reservation_len` equals the `allocation_len` of the reservation that owns
it, read from the descriptor for published and leased slots and from the
producer's own plan for reserved ones.

## Investigation log

### Q: Which is wrong, the comment or the order?

- Sources examined: `ring.rs:1267-1340` in full, `:1682-1683` for the comment text,
  `:150` for the field type, and the complete `reservation_len` grep.
- Findings: the code order is unambiguous, and the comment describes the opposite
  order. Storing `reservation_len` before the CAS would be a different hazard,
  because the slot is not yet owned by this producer at that point, so the two
  candidate fixes are not symmetric. Nothing in the file explains the choice.
- Missing evidence: no commit message, comment, or plan requirement establishes
  which of the two the author intended.
- Conclusion: unresolved, needs the author. What is settled is that the comment is
  false as written, and a future reader relying on it would be misled. That alone
  is the reportable finding, and it needs no answer to this question.

### Q: Are `conservation()` and `probe()` test-only? If any cross-process production path calls them, this moves from latent to live.

- Sources examined: a grep for `.conservation()` across `crates/` and
  `packages/`, returning `ring.rs:1887` plus nine call sites in
  `crates/shm-transport/tests/ring.rs`; a grep for `probe` across `crates/`;
  `ring.rs:1887-1892` (`Ring::probe`); and at `9c1eb4d1`
  `crates/host-runtime/src/shm_provider.rs:143-147` (`ShmRecoveryBackend::probe`),
  `crates/host-runtime/src/provider_recovery.rs:113` (the `probe` trait method) and
  `:530` (its only production call site, inside `resolve_readiness`), all three
  deleted by `ed487e11`.
- Findings: `Ring::conservation()` is called from exactly one non-test place,
  `Ring::probe()`. `Ring::probe()` has no non-test caller. The recovery
  controller's `probe()` at `provider_recovery.rs:530` dispatched to
  `ShmRecoveryBackend::probe`, which returned a constant `true` and never
  constructed or consulted a `Ring`; `ed487e11` deleted both, removing the caller
  rather than the gap. The test at
  `crates/shm-transport/tests/ring.rs:201`
  (`probe_reads_shared_state_without_consuming_a_frame`) is the only exercise of
  `Ring::probe`.
- Missing evidence: whether a future provider is intended to call
  `Ring::probe()` as its readiness probe. The comment at `shm_provider.rs:144-146`,
  deleted by `ed487e11`, argued it was unnecessary for the provider side, which
  was an argument about the single-thread ownership model rather than a permanent
  decision.
- Conclusion: resolved at this commit. Both are effectively test-only, so the
  property stays latent and its priority is lower than the raw contradiction
  suggests. It becomes live if any cross-process readiness or observability path
  starts calling `conservation()`.
  At HEAD: the grep returns `Ring::probe` (`:1891`) plus `Ring::attach` (`:1148`, through `conservation_inner`), and six `conservation()` calls in `crates/shm-transport/tests/ring.rs` rather than nine.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 23, `ring.rs:934-935` now `ring.rs:1682-1683`: The quoted lines no longer exist: the comment reads "The reservation length is assigned before any non-free state becomes visible" and the load is safe code (`let len = slot.reservation_len.load(Ordering::Relaxed);`), so the SAFETY wording and the unsafe block are gone while the false ordering claim survives.
  - line 40, `:935` now `:1683`: It is no longer the only reader: `validate_idle_window` also loads `reservation_len` with Acquire at `:1837` and compares it against the descriptor's `allocation_len` (`:1851`), and the grep now returns ten non-test sites rather than six.
  - line 90, `:703`: The window no longer contains any atomic load: `verified_producer_cursors` (`:1287-1289`) reads `arena_write` and `arena_reclaimed` before the exchange, so only the `SpanPlan::reserve` call sits between the exchange and the store.
  - line 96, `ring.rs:1004` now `ring.rs:1887`: The same walk now has a production caller through a different entry point: `Ring::attach` calls `conservation_inner(true)` at `:1148`, so a peer-broken mapping is refused at attach time even though `conservation()` itself stays probe-only.
  - line 138, `ring.rs:1004` now `ring.rs:1887`: At HEAD the grep returns `Ring::probe` (`:1891`) plus `Ring::attach` (`:1148`, through `conservation_inner`), and six `conservation()` calls in `crates/shm-transport/tests/ring.rs` rather than nine.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 90, `:703` (atomic cursor load inside the window): no replacement at HEAD.
  - line 90, `:705` (second atomic cursor load inside the window): Same cause as the previous entry; both cursor loads moved above the compare_exchange.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
