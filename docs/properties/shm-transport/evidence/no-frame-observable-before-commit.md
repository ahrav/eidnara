# no-frame-observable-before-commit

## Discovery trigger

Reading `try_reserve` showed that the producer receives writable arena spans at
reservation time, long before commit. The payload bytes a receiver would read are
therefore already present and already mutable while the frame is officially
invisible. That inverts the usual question: the guarantee cannot be "the bytes are
not there yet", it has to be "no descriptor path reaches those bytes yet". The
round-trip tests assert the positive direction only, so nothing pins the negative.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1267-1340` `try_reserve` — claims the
  slot with a `SLOT_FREE → SLOT_PRODUCER_RESERVED` compare-exchange at `:1302-1311`,
  then hands back a `ProducerReservation` at `:1331-1339`. The slot sits in
  `PRODUCER_RESERVED` for the whole write phase.
- `ring.rs:2388-2420` `write_reservation` — copies caller bytes straight into the
  arena during that phase. Nothing gates those bytes behind commit.
- `ring.rs:1395-1472` `try_receive` — the receive admission test is two lines:
  `let consumed = ... consumed.load(Ordering::Relaxed)` (`:1416`) and
  `let published = ... published.load(Ordering::Acquire)` (`:1423`), then
  `if consumed == published { return Ok(None); }` (`:1424-1426`). `published` is the
  only value that can admit a sequence.
- `ring.rs:1431-1438` — the second gate. The receiver must win
  `compare_exchange(SLOT_PUBLISHED, SLOT_RECEIVER_HELD, AcqRel, Acquire)`. A slot
  in `PRODUCER_RESERVED` fails it and yields `RingError::InvalidSharedState`.
- `ring.rs:2364-2369` `commit_reservation` publication block — the descriptor
  `write_volatile` (`:2364`), `state.store(SLOT_PUBLISHED, Relaxed)` (`:2365`),
  `arena_write.store(Relaxed)` (`:2366`), and `published.store(sequence, Release)`
  (`:2368`). The `published` store is the last write, so no earlier step can admit
  the sequence.
- `ring.rs:2536-2570` `commit` — every failure branch (`Aborted`,
  `CommitOutsideReservation`, `Underfill`, and any `commit_reservation` error)
  routes through `abort_reservation` at `:2547`, `:2552`, `:2563` before returning,
  and `abort_reservation` (`:2271-2282`) stores `SLOT_FREE` without ever touching
  `published`.
  At HEAD: `published` is advanced by `Self::advance_cursor`, an `AcqRel` compare-exchange, and it is still the last cursor written in the publication block.
  At HEAD: `arena_write` is advanced by `Self::advance_cursor`, an `AcqRel` compare-exchange against the handle's own record, not a relaxed store.
  At HEAD: `published` now arrives from `verified_published` (`ring.rs:1976-1987`), which loads it with `Ordering::Acquire` and refuses a value below the highest already seen.
  At HEAD: `consumed` now arrives from `verified_consumer_cursors` (`ring.rs:1990-2000`), which loads it with `Ordering::Acquire` and checks it against this handle's own record.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. The producer calls `try_reserve`; the slot moves to `PRODUCER_RESERVED` and the
   arena span is handed out.
2. The producer writes the full body into the arena.
3. Before commit, the receiver calls `try_receive` (directly or after a `wait_for_data` wake, `ring.rs:1476`).
4. If the receive gate were ever widened to consult slot state, `arena_write`, or
   `reservation_len` instead of `published`, the receiver would win a CAS against
   a `PRODUCER_RESERVED` slot, read a descriptor that `commit_reservation` has not
   written yet (residual bytes from the previous lap, since only
   `reservation_len`, `completion_sequence`, and `state` are reset on reclaim at
   `ring.rs:2138-2140`), and lease a span derived from stale metadata.
5. Consequence: a lease over arena bytes the producer still owns and is still
   writing, which is exactly the read-write race the zero-copy contract forbids.

## Timing windows and dependencies

The window is the entire reservation lifetime: from the CAS at `ring.rs:1302-1311`
until either `published.store(Release)` at `:2368` or `abort_reservation` at
`:2280`. It is unbounded in principle, and in practice as long as the producer's
serialization takes; `reserve_until` (`:1345-1390`) can also hold a caller parked
on the capacity doorbell inside the window while a different sequence is
outstanding. No configuration
dependency and no platform gating: both gates are plain loads and a
compare-exchange on every target. This property is the precondition for
`no-rust-reference-over-peer-writable-payload` — if a frame can be leased before
commit, that record's spans point at producer-owned memory. It is distinct from
`publication-visibility-derives-only-from-the-published-cursor`: that record is
about which edge carries *visibility* of already-published fields, this one is
about which value grants *admission* at all.

## What a test must construct

Two concurrent parties on one mapping (fault class F4, absent today). Producer:
`try_reserve`, write the full body, then park without committing. Receiver: repeatedly call
`try_receive` for a bounded window and assert every call returns `Ok(None)` — not
`Err`, since an error here would mean the CAS was attempted and lost. Then a
direct-state assertion: with the reservation open, walk the slots and assert none
in `PRODUCER_RESERVED` is reachable through the receive path, and assert
`published` is unchanged from its pre-reserve value. Finally commit and assert the
frame becomes receivable exactly once. A second arm should abort instead of
committing and assert the frame never becomes receivable. Coverage check to emit:
`shm_reservation_open_while_peer_polled`.

## Investigation log

### Q: Is `published` genuinely the only value that admits a frame to the receive path?

The catalog records no open question for this property. The question actually
investigated is the one its `high` confidence rests on.

- Sources examined: `ring.rs:1395-1472` (`try_receive`, read in full),
  `ring.rs:2364-2369` (`commit_reservation` publication order), `ring.rs:2271-2282`
  (`abort_reservation`), `ring.rs:2536-2570` (`commit` failure routing),
  `ring.rs:1267-1340` (`try_reserve`).
- Findings: `try_receive` reads exactly four shared values before it can claim a
  slot — `quarantined` via `is_quarantined` (`:1396`), `active_leases` (`:1416`),
  `consumed` (`:1416`), `published` (`:1423`). Of these only `published` can advance
  the admissible sequence; the other three can only refuse. The slot CAS at
  `:1431-1438` then requires `SLOT_PUBLISHED` exactly. `published.store` appears in
  exactly one place, `ring.rs:2368`, and it is the final write of the publication
  block.
- Missing evidence: none for the admission claim. The *visibility* of the
  descriptor fields after admission rests on the relaxed `state` store at `:2365`
  and is owned by `publication-visibility-derives-only-from-the-published-cursor`,
  not resolved here.
- Conclusion: resolved with answer — `published` is the sole admission value, and
  commit writes it last, so the property holds by construction at this commit. It
  stays cataloged because no test asserts the negative and a one-line change to
  the gate at `:1424-1426` would not fail anything.
  At HEAD: `published` is written in exactly one place at HEAD, the `advance_cursor` compare-exchange at `ring.rs:2368`, and it is the last cursor written before the wake.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 21, `:1070` now `:1416`: `consumed` now arrives from `verified_consumer_cursors` (`ring.rs:1990-2000`), which loads it with `Ordering::Acquire` and checks it against this handle's own record.
  - line 22, `:1072` now `:1423`: `published` now arrives from `verified_published` (`ring.rs:1976-1987`), which loads it with `Ordering::Acquire` and refuses a value below the highest already seen.
  - line 30, `:1619` now `:2366`: `arena_write` is advanced by `Self::advance_cursor`, an `AcqRel` compare-exchange against the handle's own record, not a relaxed store.
  - line 31, `:1620` now `:2368`: `published` is advanced by `Self::advance_cursor`, an `AcqRel` compare-exchange, and it is still the last cursor written in the publication block.
  - line 99, `ring.rs:1620` now `ring.rs:2368`: `published` is written in exactly one place at HEAD, the `advance_cursor` compare-exchange at `ring.rs:2368`, and it is the last cursor written before the wake.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
