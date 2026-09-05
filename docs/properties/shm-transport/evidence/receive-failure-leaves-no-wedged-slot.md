# receive-failure-leaves-no-wedged-slot

## Discovery trigger

`try_receive` claims the slot before it validates anything. That makes every failure
after the compare-exchange a cleanup obligation, and the code discharges that
obligation on exactly one of its failure paths. Tracing which errors quarantine and
which merely propagate with `?` showed that `enter_quarantine` appears once inside
`try_receive`, on the descriptor-validation path only. Everything downstream of that
returns an error while leaving shared state advanced.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1430-1438` — the claim happens first:
  `compare_exchange(SLOT_PUBLISHED, SLOT_RECEIVER_HELD, AcqRel, Acquire)`. From here
  on the slot is out of the producer's reach.
- `ring.rs:1442-1445` — the only cleanup path. Validation failure calls
  `self.enter_quarantine()` (`:1401`) and returns `RingError::Descriptor` (`:1445`).
  Verified by inspection: `enter_quarantine` is called exactly once inside
  `try_receive`, at `:1401`.
- `ring.rs:1446-1451` — **failure path 1.** Both `lease_span` calls propagate with `?`
  (`:1446`, `:1448`). No quarantine. At this moment the slot is `RECEIVER_HELD` and
  `consumed` has not been advanced.
- `ring.rs:1452-1454` — the commit point for consumer state, all three writes in one
  unsafe block: `state.store(SLOT_RECEIVER_LEASED, Release)`,
  `consumed.store(sequence, Release)`, `active_leases.fetch_add(1, Relaxed)`.
- `ring.rs:1460-1461` — **failure path 2**, which the catalog record does not name:
  `usize::try_from(validated.body_len()).map_err(|_| RingError::InvalidLayout)?`
  runs *after* the block above. A failure here leaves the cursor advanced and the
  lease count incremented with no lease object in existence.
- `ring.rs:1462-1470` — **failure path 3.** `ReceiveLease::new(...)` with
  `.map_err(RingError::Lease)?` at `:1470`, also after the commit point.
- `ring.rs:1427-1438` — why path 1 is permanent: the next `try_receive` recomputes
  `sequence = consumed + 1` (`:1427-1429`), the same value, and its CAS expects
  `SLOT_PUBLISHED` but finds `SLOT_RECEIVER_HELD`, so it returns
  `RingError::InvalidSharedState` (`:1438`) forever, with `is_quarantined()` false.
- `ring.rs:2070-2151` — why paths 2 and 3 are permanent: the producer's
  `reclaim_completed` breaks at the first slot whose `completion_sequence` does not
  match (`:2090-2092`), and no release will ever run for this sequence, so
  reclamation stalls there and one lease of `max_leases` is consumed for good
  (`:1417-1422` then reports saturation as ordinary backpressure).
- `ring.rs:2057-2068` `lease_span` — its four failure modes: two
  `usize::try_from` conversions (`:2058`, `:2059`), a `checked_add` overflow
  (`:617-619`), and `end > self.arena_bytes()` (`:620-622`), plus
  `LeaseSpan::new`'s null-pointer check at `crates/shm-transport/src/lease.rs:34-42`.
- `crates/shm-transport/src/lease.rs:270-276` — `ReceiveLease::new`'s only
  rejection: `span_count` outside `1..=2`, `spans[0]` none, `span_count == 1` with
  `spans[1]` some, or `span_count == 2` with `spans[1]` none.
- `crates/shm-transport/src/descriptor.rs:252-334` `validate` — the constraints
  that decide reachability: `body_len > MAX_FRAME_BYTES` rejected (`:272-274`);
  `span_count` restricted to `1..=MAX_SPANS` where `MAX_SPANS = 2`
  (`descriptor.rs:23`, `:282-284`); `spans[0].offset + spans[0].len > arena_bytes`
  rejected (`:289-295`); and for `span_count == 2`, `spans[1].offset != 0` or
  `spans[1].len > arena_bytes` rejected (`:310-319`).

## Failure scenario

Path 1, the wedge:

1. A frame is published; `try_receive` wins the CAS at `:1431-1438`; the slot is
   `RECEIVER_HELD`.
2. Validation passes at `:1442`.
3. `lease_span` fails at `:1446`. The error propagates out of `try_receive`.
4. The slot stays `RECEIVER_HELD`; `consumed` is unchanged; `quarantined` is 0.
5. Every later `try_receive` recomputes the same sequence, loses the CAS, and
   returns `InvalidSharedState`. The channel is dead.
6. Consequence: on the host this surfaces as
   `ReadClose::Corrupt("shared-memory receive failed")`
   (`crates/host-runtime/src/ring_transport.rs:677`), which ends the generation
   through the uniform `ReadClose` error path (`:635-646`; the former unclean
   classification and suspect report were deleted with `shm_provider.rs`) — but
   the ring itself is never quarantined,
   so `conservation()` still reports ordinary counts and no charge is retained as
   quarantined.

Paths 2 and 3, the unreleasable lease:

1. Same claim, same successful validation.
2. The consumer state block at `:1452-1454` commits: state `RECEIVER_LEASED`,
   `consumed` advanced, `active_leases` incremented.
3. `body_len` conversion (`:1460`) or `ReceiveLease::new` (`:1462`) fails.
4. No `ReceiveLease` exists, so nothing will ever call release for this sequence.
5. Consequence: `reclaim_completed` head-of-line blocks at this sequence forever, the
   arena bytes behind it are never reclaimed, and one lease slot is permanently
   consumed. Unlike path 1, later receives still succeed, so the loss is silent
   until the arena or the lease set runs out and reports backpressure.

## Timing windows and dependencies

There is no race here — the window is a straight-line region of one function,
entered on every successful receive: `ring.rs:1438` through `:1471`. What makes it
hard is not timing but reachability, because the failing conditions are all
implied by `validate` on a 64-bit target (see the investigation log). No
configuration dependency. Platform gating is the interesting axis: the
`usize::try_from` conversions at `:1460`, `:2058`, and `:2059` are the only failure
modes whose reachability is architecture-dependent at all, and with
`MAX_FRAME_BYTES = 64 MiB` (`crates/shm-transport/src/arena.rs:4`) they are
unreachable on 32-bit as well. Relationship: this record shares its arbitrating CAS
with `release-exactly-once-per-sequence`, approached from the receive side, and it
is the receive-side counterpart to `crashed-producer-does-not-wedge-the-sequence`
— both end with a slot stranded in a non-`FREE` state that reports as backpressure
or as a generic error rather than as a fault.

## What a test must construct

The enabling state is ordinary: one published, valid frame. The fault is a forced
failure at a named internal point after the receive CAS has succeeded — fault class
F3, which does not exist in this repository today. Two injection points are needed,
one before the consumer state block (inside `lease_span`, to reach path 1) and one
after it (at `ReceiveLease::new` or the `body_len` conversion, to reach paths 2 and
3), because the two have different post-states and different oracles. Path 1 oracle:
after the `Err`, assert `is_quarantined()` is true, or assert no slot is left in
`RECEIVER_HELD` with `consumed` un-advanced — and then assert the stronger
consequence, that a following `try_receive` on the same ring can still make
progress. Path 2 and 3 oracle: assert `active_leases` returned to its prior value
and that `reclaim_completed` can still advance past this sequence. If instead the
decision is that these paths are unreachable, the test becomes a debug assertion or
an `unreachable` marker at each site rather than an injected fault. Coverage check to
emit: `shm_receive_cas_won_then_validation_ran`.

## Investigation log

### Q: Are the two paths genuinely unreachable given `validate`?

- Sources examined: `ring.rs:1395-1472` (`try_receive` in full), `:2057-2068`
  (`lease_span`), `crates/shm-transport/src/lease.rs:34-42` (`LeaseSpan::new`)
  and `:262-276` (`ReceiveLease::new`),
  `crates/shm-transport/src/descriptor.rs:252-334` (`validate` in full) and
  `:23` (`MAX_SPANS`), `crates/shm-transport/src/arena.rs:4`
  (`MAX_FRAME_BYTES`).
- Findings: three separate results.
  *`lease_span` is unreachable given `validate`.* The two `usize::try_from` calls
  cannot fail on a 64-bit target. The `checked_add` cannot overflow because both
  operands are bounded by `arena_bytes`. The `end > arena_bytes()` check is already
  proved for span 0 by `descriptor.rs:289-295`, and for span 1 by
  `descriptor.rs:310-319`, which forces `spans[1].offset == 0` and
  `spans[1].len <= arena_bytes`, so `end == spans[1].len <= arena_bytes`.
  `LeaseSpan::new` rejects only a null base, and the base is
  `mapping.base.as_ptr().add(layout.arena + offset)` on a `NonNull` mapping with
  `offset` inside the arena.
  *`ReceiveLease::new` is unreachable given `validate` plus how `try_receive`
  builds its arguments.* `validate` constrains `span_count` to `1..=2`
  (`descriptor.rs:282-284` with `MAX_SPANS = 2`), and `try_receive` passes
  `[Some(first), second]` where `second` is `Some` exactly when
  `validated.span_count() == 2` (`ring.rs:1447-1451`). All four rejection disjuncts
  at `lease.rs:270-273` are therefore false.
  *A third path exists that the catalog record does not name.* The
  `usize::try_from(validated.body_len())` at `ring.rs:1460-1461` runs after the
  consumer state block at `:1452-1454` and has the same wedge shape as path 3. It is
  also unreachable, because `validate` caps `body_len` at
  `MAX_FRAME_BYTES = 64 MiB` (`descriptor.rs:272-274`,
  `arena.rs:4`), which fits `usize` on every supported target.
- Missing evidence: nothing needed for the reachability question. What is missing is
  any statement of this reasoning in the code — no debug assertion, no
  `unreachable` marker, no comment at `ring.rs:1446`, `:1460`, or `:1470` records that
  these errors are prevented upstream. The derivation depends on `validate` keeping
  four specific invariants, and nothing links the two functions.
- Conclusion: resolved with answer — all three paths are unreachable at this commit,
  given `validate` and a target where `u64` values bounded by 64 MiB convert to
  `usize`. That downgrades the record from a live wedge to a latent one, and it
  changes what the test should be: not an injected fault to prove the wedge, but a
  guard at each site so a future relaxation of `validate` fails loudly instead of
  silently wedging the channel. The `always-or-unreached` check semantics the
  catalog chose are the right ones, and the companion obligation is a reachability
  check on the paths themselves.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 17, `ring.rs:1095-1101` now `ring.rs:1442-1445`: `try_receive_inner` no longer quarantines on the validation path; `try_receive` maps every error it returns through `quarantine_with` (`:1401`), so all three failure paths quarantine at HEAD.
  - line 20, `:1098` now `:1401`: `enter_quarantine` no longer appears anywhere inside the receive path; quarantine comes from the uniform `quarantine_with` wrapper in `try_receive`, which covers every failure, not just descriptor validation.
  - line 24, `ring.rs:1114-1118` now `ring.rs:1452-1454`: The three writes are not inside an `unsafe` block at HEAD, and the two cursor writes are `advance_cursor` compare-exchanges rather than plain stores, followed by a local mirror at `:1455-1458`.
  - line 36, `:1090` now `:1438`: `try_receive` quarantines that error through `quarantine_with` (`:1401`), so `is_quarantined()` becomes true rather than staying false.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
