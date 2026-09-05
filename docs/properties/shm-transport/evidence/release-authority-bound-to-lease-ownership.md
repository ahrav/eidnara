# release-authority-bound-to-lease-ownership

## Citation refresh, 2026-08-30

The ring-transport refactor (`0f336d3c`, `d8bde128`, `793a973e`, `ed487e11`)
renamed `crates/host-runtime/src/shm_provider.rs` to
`crates/host-runtime/src/ring_transport.rs` and deleted `provider_recovery.rs`,
`transport_negotiation.rs`, and `transport_provider.rs`. Host-side citations below
were re-anchored against `ring_transport.rs` at `e447c927`.

Where the cited construct survives, the citation names `ring_transport.rs` and a
line re-verified against that commit. Where it does not, the original reference is
kept and prefixed `former`, so it reads as pre-refactor evidence rather than a
current location. A `former` line number is never a claim about the tree today.
Every `provider_recovery.rs` reference is `former` by definition: that module has
no successor. See the refresh note in [../catalog.md](../catalog.md).

Citations below were re-verified 2026-08-31 against post-#131 HEAD (merge
`5d638e3e8`, eventfd doorbell rewrite of `ring.rs` and the native addon); stale
pre-rewrite line numbers were corrected in place.

## Discovery trigger

Reading the two signatures next to each other. `ProducerReservation::commit`
returns a `ReleaseIdentity` to the producer, and `Ring::release` accepts a
`ReleaseIdentity` from anyone holding a `&Ring`. Nothing on the release path
establishes that the caller is the party that took the lease. The authority to
complete a frame is therefore carried by a value, not by ownership of the lease.

## Evidence trail

Both signatures confirmed by direct read at this commit:

```rust
// crates/shm-transport/src/backend/ring.rs:1528
pub fn release(&self, identity: ReleaseIdentity) -> Result<(), LeaseError>
```

```rust
// crates/shm-transport/src/backend/ring.rs:2536
pub fn commit(mut self, body_len: usize) -> Result<ReleaseIdentity, ProducerError>
```

- `ring.rs:1528-1600` — the whole of `release`. Its checks are: quarantine (`:1529`),
  `identity.incarnation() != self.grant.incarnation` (`:1537`),
  `identity.lane() != self.grant.lane` (`:1540`), `sequence == 0` (`:1544`),
  `sequence > consumed` (`:1556`), and three descriptor-versus-identity comparisons
  (`:1566`, `:1569`, `:1572`). Then the arbitrating compare-exchange
  `SLOT_RECEIVER_LEASED → SLOT_RELEASE_PENDING` at `:1575-1580`. There is no role
  check, no owner check, and no lease token — the `&self` receiver is the only
  capability required.
- `ring.rs:2318` — `commit_reservation` builds
  `ReleaseIdentity::new(self.grant.incarnation, self.grant.lane, sequence)` and
  returns it (`:2385`), so the identity the producer receives is byte-identical to
  the one `try_receive` derives for the receiver at `:1441`.
- `ring.rs:1591-1597` — a successful release stores `completion_sequence` and
  decrements `active_leases` while the receiver's `ReceiveLease` is still alive and
  still holds `LeaseSpan` pointers into the arena.
- `ring.rs:2070-2151` `reclaim_completed` — the producer's next `try_reserve` calls
  it first (`:1281`), and it advances `arena_reclaimed` (`:2143`) and sets the
  slot `SLOT_FREE` (`:2140`) for any slot whose `completion_sequence` matches. So a
  premature release makes those exact bytes reservable.
- `crates/shm-transport/src/lease.rs:366-372` — the receiver's own `Drop` then
  calls `release_once()` and `let _ = ...` discards the `DuplicateRelease`
  (`:369`), so the legitimate holder is never told its lease was completed by
  someone else. This is the same discard site as `release-failure-is-observable`.
- Existing checks, corrected: the identity-validation ladder is
  `crates/shm-transport/src/backend/ring.rs:3871-3907`; lines `:177-180` (source tree; not at HEAD) are the
  following `ProducerError::Exhausted` assert.
  Within that ladder, `first_id` comes from `first.commit(first_len)` (`:140` (source tree; not at HEAD)) and
  the lease over that sequence is live from `crates/shm-transport/src/backend/ring.rs:3901` until `crates/shm-transport/src/backend/ring.rs:3905`; the test calls
  `ring.release` at `crates/shm-transport/src/backend/ring.rs:3903`, `crates/shm-transport/src/backend/ring.rs:3903`, and `crates/shm-transport/src/backend/ring.rs:3903` with *mutated* copies of `first_id`
  to elicit `WrongIncarnation`, `WrongLane`, and `InvalidSequence`. The violating
  call is one unmutated argument away from an existing test.
  `crates/shm-transport/src/backend/ring.rs:3910` `stale_lap_release_cannot_complete_recycled_slot` is
  confirmed and is a genuine full-lap test.

## Failure scenario

Within one process holding a single `Ring` used in both directions:

1. Producer: `try_reserve`, write, `let id = reservation.commit(n)?` — `id` is now
   in producer-side hands (`ring.rs:2385`).
2. Receiver: `let lease = ring.try_receive()?` — the slot is `RECEIVER_LEASED`,
   `active_leases` is 1, and `lease` holds raw `LeaseSpan` pointers
   (`ring.rs:1446-1470`).
3. Producer: `ring.release(id)`. Every check at `:1537-1574` passes, because the
   descriptor genuinely carries that incarnation, lane, and sequence. The CAS at
   `:1575-1580` sees `SLOT_RECEIVER_LEASED` and succeeds. `completion_sequence` is
   published and `active_leases` drops to 0 (`:1591-1597`).
4. Producer: `try_reserve` again. `reclaim_completed` (`:1281`, `:2070-2151`) sees
   the matching `completion_sequence`, advances `arena_reclaimed`, frees the slot.
5. Producer writes the new frame's body into the reclaimed span
   (`write_reservation`, `:2388-2420`) — the same bytes the live lease still points
   at.
6. Receiver reads through `lease.segment(i).read_byte(..)` or `to_vec()` and
   observes the new frame's bytes, or a torn mixture.
7. `lease` drops; `release_once` returns `DuplicateRelease`, discarded at
   `lease.rs:369`. Nothing anywhere reports that anything went wrong.

## Timing windows and dependencies

The window opens when `try_receive` sets `SLOT_RECEIVER_LEASED` (`ring.rs:1452`) and
closes when the lease's own release runs. Within it, one call — `Ring::release` with
a value the API returned to the producer — is sufficient; no race, no malformed
input, no memory corruption. The read-after-recycle needs one further step, the
producer's next `try_reserve`, which is unconditional in a busy channel because
`reclaim_completed` runs at the top of it. No configuration dependency
(`HostConfig.liveness` is irrelevant here), no platform gating: this is plain
compare-exchange logic that behaves identically everywhere. Relationship:
`release-exactly-once-per-sequence` dominates this record only for *duplicate*
releases — it says nothing about a *first* release by the wrong party, which is the
actual gap. `no-rust-reference-over-peer-writable-payload` is what keeps the
consequence a stale read rather than immediate undefined behaviour, since the lease
exposes raw pointers and volatile reads rather than a `&[u8]`.

## Reachability in the shipped two-process topology

Not reachable. It is reachable only in a same-process arrangement where one `Ring`
serves as both producer and receiver, which today means the transport's own tests.
Three independent facts establish this:

1. **No non-test caller retains the identity `commit` returns.** Every non-test
   `commit` call site discards it: `crates/host-runtime/src/ring_transport.rs:823` and
   `:823` (`reservation.commit(body_len).map_err(|_| ())?;`),
   `packages/shm-native/src/lib.rs:1074-1077` and `:1205-1208`
   (`.map_err(|_| error(...))?;` followed by `Ok(())`), and the public-but-test-only
   client endpoint (formerly `TestShmPeer::send`, now `RingClientEndpoint::send`)
   at `ring_transport.rs:885` (commit at `:930`).
2. **The only non-test direct `Ring::release` call is a receiver's own.** A search
   of `crates/` and `packages/` for `Ring::release` call sites yields exactly two
   outside tests and benches: `ring.rs:2438` inside `ring_release_callback`
   (`:2436-2440`), which is the lease's own release path, and
   `packages/shm-native/src/lib.rs:327-331` (source tree; not at HEAD),
   `channel.from_host.release(active.identity)` inside `detach_active`
   (`:332-357`). The second is on the addon's *receive* ring, with an identity
   captured from `lease.identity()` at `:1236` (source tree; not at HEAD) and stored in the addon's `active`
   table because `poll` calls `std::mem::forget(lease)` at `:1256` (source tree; not at HEAD). That is the
   legitimate holder completing its own lease through a different bookkeeping
   route. The addon never calls `release` on `to_host`.
3. **The two directions are separate objects with separate identities.**
   `DuplexRing::create` (`ring.rs:2604-2612`) builds `first` with lane 0 and
   `second` with lane 1, each through `Ring::create_in`, which draws
   `incarnation = Incarnation::random()` at `:1051`. So even a producer that did
   retain an identity from `commit` on its send ring would be rejected on its
   receive ring by `ring.rs:1537-1542` with `WrongIncarnation` or `WrongLane`. The
   addon's `to_host`/`from_host` (`lib.rs:77-78`) and the host's
   `rings.first`/`rings.second` (former `shm_provider.rs:597`, former `:555-557`) both follow this
   split, and no non-test path reserves and receives on the same `Ring`.

Severity therefore: a latent API-shape hazard, not a live defect in the shipped
topology. What keeps it worth a record is that the composition is available rather
than prevented — `Ring` and `Ring::release` are public, `commit` hands the identity
out, and the type system does not distinguish a produce-direction `Ring` from a
receive-direction one. The property protects a boundary that is currently held by
call-site convention in two separate codebases.

## What a test must construct

The enabling state is one `Ring` bound in both roles, which is the transport's own
unit-test shape: `Ring::create(&profile(), lane)`, publish, `commit` and keep the
returned identity, `try_receive` and keep the lease alive. Then the fault is a
single ordinary call, `ring.release(producer_identity)`, with no injection needed.
Assert: the call fails; the slot is still `RECEIVER_LEASED` in `conservation()`;
`active_leases` is still 1; and a following `try_reserve` cannot allocate the bytes
under the live lease. To demonstrate the consequence rather than the guard, seed the
lease's first span with a known byte, run the premature release plus a second
`try_reserve` and write, and assert the lease still reads the original byte. A
two-process arm is not required for the property and cannot construct it today for
the reasons above; if the answer to the open question is that `Ring::release`
should not be public, the test becomes a compile-fail assertion instead.

## Investigation log

### Q: Is `Ring::release` intended to be public at all, or should completion be reachable only through `ReceiveLease`?

- Sources examined: `ring.rs:1528-1600` (`release`), `:2436-2440`
  (`ring_release_callback`), `:2536-2570` (`commit`), `:2604-2612`
  (`DuplexRing::create`), `:1040-1091` (`create`/`create_in` and the random
  incarnation); `crates/shm-transport/src/lease.rs:324-372`
  (`ReceiveLease::release`, `release_once`, `Drop`);
  `packages/shm-native/src/lib.rs:77-78`, `:332-357`, `:1394-1500`;
  `crates/host-runtime/src/ring_transport.rs:664-747`, `:846-933`; a
  repository-wide search of `crates/` and `packages/` for `.release(` call sites.
- Findings: the reachability half is resolved — see the section above. The
  *reason* the method is public is also established: the addon needs a
  lease-independent completion path because `poll` `mem::forget`s the
  `ReceiveLease` at `lib.rs:1256` (source tree; not at HEAD) and re-derives completion from its own `active`
  table at `:332-357`, so making completion reachable only through `ReceiveLease`
  would require the addon to keep the Rust lease alive across the N-API boundary.
  That is a real design constraint, not an accident.
- Missing evidence: whether the public method is *intended* as a general
  completion entry point or as an internal detail the addon happens to need.
  `docs/shm-transport.md` describes the ownership contract in terms of
  leases and does not mention a direct release entry point. No commit message,
  plan requirement (R1-R19, AE1-AE15), or doc comment states an intended caller
  set; the doc comment at `ring.rs:1514` reads only "Validates and records one
  explicit completion", which does not name a caller.
- Conclusion: partially resolved. The reachability sub-question is answered with
  evidence: producer-side release is not reachable in the shipped two-process
  topology and is reachable only in a same-process, single-`Ring` arrangement,
  which downgrades this from a live defect to a latent hazard. The design intent
  sub-question needs human input, because the answer determines whether the test
  above asserts a runtime rejection or whether the correct outcome is that the
  composition stops being expressible at all.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 35, crates/shm-transport/src/backend/ring.rs:1175 now crates/shm-transport/src/backend/ring.rs:1528: At HEAD `release` is `pub(crate) fn release`, reachable only through `ReceiveLease` via the crate-private `ReleaseSink` trait, so holding a `&Ring` is no longer a public capability to complete a frame.
  - line 44, `ring.rs:1175-1247` now `ring.rs:1528-1600`: The checks now live in `release_inner`, they include an `active_leases == 0` rejection at `:1559`, every failure quarantines through `inspect_err(|_| self.enter_quarantine())` at `:1533`, and `release` is `pub(crate)`.
  - line 68, `crates/shm-transport/tests/ring.rs:152-175` now `crates/shm-transport/src/backend/ring.rs:3871-3907`: The ladder moved into the `ring.rs` unit tests as the table-driven `mismatched_release_identity_names_the_field_and_quarantines`, which adds a fourth `DuplicateRelease` case and asserts every mismatch quarantines; `crates/shm-transport/tests/ring.rs` cannot call `Ring::release` at all now that it is `pub(crate)`.
  - line 134, `packages/shm-native/src/lib.rs:327-331`: The addon holds a `ReceiveLease<'static>` inside `ActiveLease` and never calls `Ring::release`, which is `pub(crate)` at HEAD, so the only `Ring::release` caller in the tree is the `ReleaseSink` impl the lease uses.
  - line 188, `lib.rs:1256`: The addon does keep the Rust lease alive across the N-API boundary at HEAD and completes through it, and `Ring::release` is `pub(crate)`, so the design constraint that justified a public entry point no longer exists.
  - line 197, `ring.rs:1174` now `ring.rs:1514`: The doc comment now names the caller set: it says only `ReceiveLease` reaches this, because an identity is `Copy` and a public entry point would let a caller release a frame while still holding the lease that reads it (`:1514-1517`).
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 68, `:177-180` (following ProducerError::Exhausted assert): The ladder is a standalone test with nothing after it; the `ProducerError::Exhausted` assertion now lives in `retained_oldest_lease_enforces_fifo_reclamation` at `crates/shm-transport/tests/ring.rs:148-151`.
  - line 70, `:140` (first_id from first.commit(first_len)): The ladder no longer keeps the identity `commit` returned; it forges copies of the live lease's own `identity()` at `crates/shm-transport/src/backend/ring.rs:3903`.
  - line 134, `packages/shm-native/src/lib.rs:327-331` (channel.from_host.release(active.identity)): `detach_active` now completes through the stored lease with `active.lease.release()` at `packages/shm-native/src/lib.rs:350-355`.
  - line 137, `:1236` (identity captured from lease.identity() in poll): No identity is captured; `poll` moves the whole `ReceiveLease` into `channel.active` at `packages/shm-native/src/lib.rs:1445-1451`.
  - line 138, `:1256` (std::mem::forget(lease) in poll): `poll` no longer forgets the lease; it stores it in `ActiveLease` at `packages/shm-native/src/lib.rs:1445-1451`.
  - line 188, `lib.rs:1256` (poll mem::forgets the ReceiveLease): `poll` stores the `ReceiveLease<'static>` in `ActiveLease` (`packages/shm-native/src/lib.rs:50-54` and `:1445-1451`) instead of forgetting it.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
