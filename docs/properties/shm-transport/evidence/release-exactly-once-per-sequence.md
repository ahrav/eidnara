# release-exactly-once-per-sequence

## Discovery trigger

`ReleaseIdentity` is a value that several parties can hold at once — the receiver
gets one from `try_receive`, the producer gets the same one from `commit`, and the
addon copies it into its own table. Descriptor slots are reused every
`descriptor_depth` sequences. So the question is which mechanism actually arbitrates
between competing releases: the identity comparison, or something else. The
identity comparison cannot be the arbiter, because a stale identity can match a
recycled slot's descriptor bytes on a later lap.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:2039-2046` `slot_ptr` — the index is
  `(sequence - 1) % self.grant.descriptor_depth` (`:2043`). Sequences `N` and
  `N + depth` share one slot, so identity uniqueness across laps is a property of
  the descriptor contents, not of the address.
- `ring.rs:1575-1580` — the single mutation point:
  ```rust
  let changed = unsafe {
      (*slot).state.compare_exchange(
          SLOT_RECEIVER_LEASED,
          SLOT_RELEASE_PENDING,
          Ordering::AcqRel,
          Ordering::Acquire,
      )
  };
  ```
  Exactly one caller can observe `SLOT_RECEIVER_LEASED` and move the slot on. Every
  side effect of a successful release — the `completion_sequence` store and the
  `active_leases` decrement at `:1591-1593` — is downstream of it, so both inherit its
  exactly-once character.
- `ring.rs:1581-1589` — the error mapping: an observed `SLOT_RELEASE_PENDING` or
  `SLOT_FREE` becomes `LeaseError::DuplicateRelease`; anything else becomes
  `InvalidSequence`. This is the only place `DuplicateRelease` originates on the ring
  path.
- `ring.rs:1537-1574` — the checks that run *before* the CAS: a `consumed` acquire load
  (`:1550-1555`) with `sequence > consumed` rejected (`:1556`), then a
  `std::ptr::read_volatile((*slot).descriptor.get())` (`:1565`) and three field
  comparisons against the identity (`:1566`, `:1569`, `:1572`). None of these is atomic
  with the CAS at `:1575`, and the descriptor read is a separate access from the state
  transition.
- `crates/shm-transport/src/lease.rs:350-357` `release_once` — a second,
  independent guard: `if self.released { return Err(LeaseError::DuplicateRelease); }`
  (`:351-353`). So duplicates *through one lease handle* are caught locally without
  ever reaching the ring. Span re-verified at post-#131 HEAD: `:350-357`.
- `lease.rs:366-372` `Drop` — the third release entry point, discarding its result
  at `:369`. A duplicate arriving here is therefore invisible; see
  `release-failure-is-observable`.
- `ring.rs:2070-2151` `reclaim_completed` — after a successful release the producer
  resets `reservation_len`, `completion_sequence`, and `state` (`:2138-2140`) but
  **not** the descriptor body, so residual descriptor bytes from the previous lap
  survive in the slot until the next `commit_reservation` overwrites them at
  `:2364`.
- Existing checks, corrected and re-anchored at post-#131 HEAD: the
  sequential-case ladder is
  `crates/shm-transport/src/backend/ring.rs:3874-3891`; `crates/shm-transport/tests/ring.rs:148-151` is the
  following `Exhausted` assert. It covers wrong incarnation (`crates/shm-transport/src/backend/ring.rs:3874-3883`), wrong lane
  (`:3884-3887`), wrong sequence (`:3888-3891`), and duplicate (`:3892-3895`).
  `crates/shm-transport/src/backend/ring.rs:3910` `stale_lap_release_cannot_complete_recycled_slot` is confirmed
  and is a genuine full-lap test: it wraps `depth` sequences (`:3918-3921`), leases a
  fresh frame (`:3924`), and asserts the lap-old identity yields `InvalidSequence`
  (`:3925-3929`).
  At HEAD: The stale release also quarantines the ring now, and the test asserts the recycled slot stays SLOT_RECEIVER_LEASED for the fresh frame.
  At HEAD: Each identity mismatch now also quarantines the ring, and the surviving lease's own release then returns Quarantined.
  At HEAD: release_once sets `released` before calling the sink, so Drop cannot retry a failed release, and the sink is a borrowed ReleaseSink rather than a raw callback and context pair.
  At HEAD: The descriptor is read through DescriptorSlot::read_descriptor rather than an inline std::ptr::read_volatile.
  At HEAD: consumed comes from verified_consumer_cursors, which loads the shared cursors with Acquire and rejects any value that disagrees with this handle's own record.
  At HEAD: The compare-exchange runs on a &DescriptorSlot with no unsafe block, and Ring::release wraps release_inner so any failure also quarantines the ring.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

The sequential cases are covered. The uncovered one is the read-then-CAS window,
which needs a second party progressing between `:1565` and `:1575`:

1. Party A holds a stale identity for sequence `N` — for example a copy kept after
   its lease was already completed, or a lap-old identity.
2. Party A calls `release(N)`. The quarantine, incarnation, lane, and `consumed`
   checks pass. At `:1565` it reads the descriptor and, because the slot still holds
   sequence `N`'s residual bytes, the three comparisons at `:1566-1574` pass.
3. Before A reaches `:1575`, the legitimate holder of `N` releases, the producer
   runs `reclaim_completed` and frees the slot (`:2140`), then reserves, commits
   sequence `N + depth` into the same slot (`:2364-2369`), and the receiver leases
   it (`:1452`).
4. A's CAS at `:1575-1580` now observes `SLOT_RECEIVER_LEASED` — belonging to
   `N + depth`, not `N` — and succeeds.
5. `completion_sequence` is stored as `N`, not `N + depth` (`:1591`). The next
   `reclaim_completed` compares `completion != next` at `:2090` and breaks, so
   reclamation stalls; meanwhile `active_leases` has been decremented for a lease
   that is still live (`:1592-1593`).
6. Consequence: two releases counted for one sequence, one live lease with its
   accounting already returned, and a reclamation cursor that no longer advances.
   The legitimate holder's later `Drop` gets `DuplicateRelease`, discarded at
   `lease.rs:369`.

## Timing windows and dependencies

The window is the instruction span between the descriptor `read_volatile` at
`ring.rs:1565` and the compare-exchange at `:1575` — a handful of loads and branches,
so it is narrow but real, and it is entered on every release call. Constructing the
interleaving requires a *full lap* of `descriptor_depth` sequences to complete
inside it, which is why physical concurrency alone is an implausible constructor and
a deterministic scheduling point is the practical route. No configuration
dependency; no platform gating, though the `Acquire` failure ordering at `:1579` and
the `Relaxed` `active_leases` operations at `:1454` and `:1592-1593` mean a weakly-ordered
target is the honest place to run it. Relationship: this record dominates
`release-authority-bound-to-lease-ownership` only for *duplicate* releases; a first
release by the wrong party passes this property's check and is that record's
concern. `receive-failure-leaves-no-wedged-slot` shares the same CAS as its arbiter,
approached from the receive side.
At HEAD: Both the increment and the decrement of active_leases go through Ring::advance_cursor, an AcqRel compare-exchange from the handle's recorded value, so neither is a Relaxed fetch any more.

## What a test must construct

At least two release attempts for one sequence, and for the uncovered case they must
interleave. Concretely: a deterministic scheduling point immediately after the
descriptor read at `ring.rs:1565` (fault class F3, absent today), holding party A
there while a second party performs a legitimate release, a `reclaim_completed`, a
reserve, a commit that reuses the slot, and a fresh `try_receive`; then release A and
assert its CAS fails. The oracle is per-identity, not aggregate: for the multiset of
release calls carrying one identity, exactly one returns `Ok`, and `active_leases`
is decremented exactly once. Assert `active_leases` directly rather than inferring
it from `conservation()`, and assert that `completion_sequence` never holds a value
for a sequence whose lease is still live. Coverage checks to emit:
`shm_full_lap_slot_recycled` and `shm_release_raced_with_reclaim`.

## Investigation log

### Q: Is the descriptor re-read at `:1565` atomic with the arbitrating CAS at `:1575`, and does that gap admit a second successful release for one sequence?

The catalog records no open question here; the record's own Impact names this gap as
the thing to make explicit, so it is the question investigated.

- Sources examined: `ring.rs:1528-1600` (`release` in full), `:2039-2046`
  (`slot_ptr` indexing), `:2070-2151` (`reclaim_completed`, including which slot
  fields are reset at `:2138-2140`), `:2308-2386` (`commit_reservation`'s descriptor
  write), `crates/shm-transport/src/lease.rs:350-372`,
  `crates/shm-transport/src/backend/ring.rs:3868-3939` (both existing release tests).
- Findings: the two accesses are separate and nothing serializes them. The
  exactly-once guarantee for *state* is nonetheless sound, because the CAS is the
  sole mutation point and only one caller can win it. What the gap admits is a
  *misattributed* win: a caller whose pre-checks validated sequence `N` completing
  whichever lease occupies the slot at CAS time. Reaching it requires a full lap
  inside the window, which is why the existing full-lap test at `crates/shm-transport/src/backend/ring.rs:3910`
  does not catch it — that test is single-threaded, so the recycle completes long
  before the stale release is attempted and the pre-check at `:1572` correctly
  rejects it.
- Missing evidence: none about the code. What is missing is a way to run it — there
  is no failpoint after `:1565`, and the repository has no loom, Shuttle, Miri, or
  ThreadSanitizer configuration, so the interleaving cannot be constructed today
  (fault classes F3 and F4).
- Conclusion: resolved with answer — the accesses are not atomic, the exactly-once
  property on state still holds, and the residual hazard is misattribution rather
  than a double success. It remains unexercised, needs F3, and is the reason this
  record stays open despite good sequential coverage.
  At HEAD: Ring::release is pub(crate) and reachable only through ReceiveLease, so a party holding a copied ReleaseIdentity cannot call it directly.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 19, `ring.rs:1212-1219` now `ring.rs:1575-1580`: The compare-exchange runs on a &DescriptorSlot with no unsafe block, and Ring::release wraps release_inner so any failure also quarantines the ring.
  - line 39, `:1192` now `:1550-1555`: consumed comes from verified_consumer_cursors, which loads the shared cursors with Acquire and rejects any value that disagrees with this handle's own record.
  - line 40, `:1201` now `:1565`: The descriptor is read through DescriptorSlot::read_descriptor rather than an inline std::ptr::read_volatile.
  - line 44, `crates/shm-transport/src/lease.rs:184-192` now `crates/shm-transport/src/lease.rs:350-357`: release_once sets `released` before calling the sink, so Drop cannot retry a failed release, and the sink is a borrowed ReleaseSink rather than a raw callback and context pair.
  - line 58, `crates/shm-transport/tests/ring.rs:152-175` now `crates/shm-transport/src/backend/ring.rs:3874-3891`: Each identity mismatch now also quarantines the ring, and the surviving lease's own release then returns Quarantined.
  - line 64, `:219-223` now `:3925-3929`: The stale release also quarantines the ring now, and the test asserts the recycled slot stays SLOT_RECEIVER_LEASED for the fresh frame.
  - line 100, `:1117` now `:1454`: Both the increment and the decrement of active_leases go through Ring::advance_cursor, an AcqRel compare-exchange from the handle's recorded value, so neither is a Relaxed fetch any more.
  - line 128, `ring.rs:1175-1247` now `ring.rs:1528-1600`: Ring::release is pub(crate) and reachable only through ReceiveLease, so a party holding a copied ReleaseIdentity cannot call it directly.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
