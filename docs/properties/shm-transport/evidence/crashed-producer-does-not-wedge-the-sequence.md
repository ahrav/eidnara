# crashed-producer-does-not-wedge-the-sequence

## Discovery trigger

The next sequence is derived, not stored: `try_reserve` computes `published + 1`
every time. So a reservation that is claimed but never committed and never aborted
leaves the derived sequence pointing at a slot that is no longer `FREE`, and the
derivation will keep producing that same value forever. The only thing that undoes
the claim runs in a destructor, which is exactly what a killed process does not
execute.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1296-1311` — the derivation and the
  claim. `let sequence = published.checked_add(1)` (`:1296-1298`, corrected from the
  catalog's `:689-703`, whose start line lands on the `.checked_add` continuation
  rather than the `let`), then `slot_ptr(sequence)` (`:1299`) and
  `compare_exchange(SLOT_FREE, SLOT_PRODUCER_RESERVED, AcqRel, Acquire)` with
  `.map_err(|_| ProducerError::Exhausted)?` (`:1302-1311`). A losing CAS is reported as
  `Exhausted` — a backpressure code — regardless of *why* the slot was not free.
- `ring.rs:1963` — `published` is loaded `Relaxed` from the shared producer page, so
  it is the surviving process's own view of a cursor the dead process last wrote.
  Nothing else feeds the derivation.
- `ring.rs:2271-2282` `abort_reservation` — corrected span; the catalog records
  `:1154-1162`, which is now `:2271-2282`, and the prompt's `1150-1165` is wider
  than the function. It stores
  `reservation_len = 0` (`:2279`) and `state = SLOT_FREE` (`:2280`).
- **Correction to the catalog record.** The catalog says `abort_reservation` "is the
  only path that restores `SLOT_FREE`". More precisely: it is the only path that
  returns a slot from `PRODUCER_RESERVED` to `SLOT_FREE` *once a
  `ProducerReservation` handle exists*. Two other sites also store `SLOT_FREE` —
  `try_reserve`'s own rollback at `:1315` and `:1319` (the catalog's `:710` and
  `:715`), which fire on arena exhaustion
  and arena-planning errors *before* the handle is returned, and
  `reclaim_completed` at `:2140`, which frees a slot from `RELEASE_PENDING`. Neither
  helps here: the rollback path is already past, and reclaim only acts on released
  frames.
- `ring.rs:2587-2594` `impl Drop for ProducerReservation` — `if !self.finished { self.ring.abort_reservation(self.sequence); }`. This is the path a kill skips.
- `ring.rs:2520-2531` and `:2536-2570` — the other `abort_reservation` callers, all
  in-process: `write` on overflow (`:2525`), `commit` on
  `CommitOutsideReservation`, `Underfill`, and `commit_reservation` failure (`:2547`,
  `:2552`, `:2563`), and `abort` (`:2575`).
- `ring.rs:1293-1295` and `:1354` — the symptoms. Once `published - completed` reaches
  `descriptor_depth`, `try_reserve` returns `Exhausted`, and `reserve_until` converts
  sustained `Exhausted` into `ProducerError::Deadline`. Both are ordinary
  backpressure. `enter_quarantine` (`:1915-1922`) is never called on either path.
- `ring.rs:1686-1695` — why the accounting looks healthy: `conservation()` counts a
  `SLOT_PRODUCER_RESERVED` slot into `descriptors.producer_reserved` and its
  `reservation_len` into `bytes.producer_reserved`, and adds the same length to
  `charged`. The totals therefore conserve, and a stranded reservation is
  indistinguishable from a legitimately in-flight one.
- `crates/host-runtime/src/ring_transport.rs:769-772` — worth recording as the *negative*
  case: the host wraps its publish in `catch_unwind`, and a panic unwinds through
  `ProducerReservation::Drop`, so `abort_reservation` does fire. A panic is not a
  wedge. Only a path that skips destructors is — `SIGKILL`, `abort()`, or a
  `panic = "abort"` profile.
- Existing check: none, confirmed. The six kill-based tests in
  `crates/host-runtime/tests/shm_failure_modes.rs` (`:214`, `:226`, `:246` (source tree; not at HEAD), `:282` (source tree; not at HEAD),
  `:316` (source tree; not at HEAD), `:358` (source tree; not at HEAD)) all kill outside a reservation.

## Failure scenario

1. A producer calls `try_reserve`. `published` is `N-1`, so `sequence = N`, and the
   slot for `N` moves `FREE → PRODUCER_RESERVED` (`ring.rs:1302-1311`).
2. The producer writes some or all of the body.
3. The process is killed before `commit`. `ProducerReservation::Drop` never runs, so
   the slot stays `PRODUCER_RESERVED`, `reservation_len` stays non-zero, and
   `published` is still `N-1`.
4. Any later producer on the same object derives `sequence = published + 1 = N`
   again (`:1296-1298`), and its CAS at `:1302-1311` fails against the stranded slot,
   returning `Exhausted`.
5. `reserve_until` retries until the deadline and returns `Deadline` (`:1354`).
6. Consequence: that direction can never publish again. `is_quarantined()` is false,
   `conservation()` reports `producer_reserved == 1` and conserves, so no charge is
   retained and no recovery episode starts. The only signal is a code whose plain
   meaning is "try again later", which a caller will honour indefinitely.

## Timing windows and dependencies

The window is the whole reservation lifetime, `ring.rs:1302` through either
`published.store(Release)` at `:2368` or `abort_reservation` at `:2280`. In the
shipped host that span contains the entire serialization of the frame body —
`publish_direct` runs `direct.serialize` inside it
(`crates/host-runtime/src/ring_transport.rs:794-797`) and `publish_owned` performs two
writes (`:809-810`) — so it is proportional to frame size, up to
`MAX_FRAME_BYTES = 64 MiB` (`crates/shm-transport/src/arena.rs:4`). In the addon
the window is wider still and includes a JavaScript callback: `produce` holds the
reservation across `fill.call(views)` (`packages/shm-native/src/lib.rs:1049`), and
the two-phase `reserve`/`commit_reservation` pair holds it across an entire return to
JavaScript and back (`:1105-1111` to `:1205-1207`). That second shape is the practical
kill target. No configuration dependency; no platform gating beyond the
Linux-only attach path. Relationship: this is the producer-side twin of
`attach-reconciles-or-refuses-stale-shared-cursors` and shares its root — no
liveness signal and no reconciliation — and it shares with
`dead-peer-charges-are-reclaimed-or-declared` the property that the fault surfaces
as a legal code rather than as a fault.

One scoping note, stated because it changes what "any later producer" means. In the
shipped two-process topology each candidate gets a fresh `DuplexRing`
(`ring.rs:2604-2612`) with a fresh random incarnation (`:1051`), so a replacement peer
does not inherit the dead peer's object. The literal "a later producer is blocked"
sequence therefore requires a second producer on the *same* object, which today is
constructible in a same-process arrangement or through a re-attach to the same
descriptor. The shipped-topology manifestation is narrower but still real: the
surviving side holds a ring with one slot and its arena bytes permanently
unreclaimable, reported by `conservation()` as in-flight rather than as lost.

## What a test must construct

Termination during an open reservation — fault class F1 at a kill point the harness
does not yet offer. `RoleProcess::kill` (`crates/host-runtime/tests/support/shm_process.rs:257-263` (source tree; not at HEAD)),
`reap_killed` with its signal-9 assertion (`:272-292` (source tree; not at HEAD)), and `observation_window`
(`:266-269` (source tree; not at HEAD)) all exist. What is missing is a victim scenario that parks *inside* a
reservation: the five existing scenarios (`:712-749` (source tree; not at HEAD)) cannot, because
`TestShmPeer::send` performs reserve, write, and commit inside one function with no
suspension point (`crates/host-runtime/src/ring_transport.rs:901-933`). So the new scenario
must reserve directly against its `to_host` ring, write a partial body, emit a
barrier record, and park. After the kill and reap, the oracle has two arms.
Arm 1, the property as stated: a replacement producer on the same object must
eventually publish, or the failure must be reported as something other than
`Exhausted`/`Deadline` — assert on the *error variant*, because a test that only
checks "reserve failed" passes on both the wedged and the healthy-backpressure case.
Arm 2, the shipped-topology consequence: assert `conservation()` still reports the
stranded slot as `producer_reserved` and that the arena bytes never return to `free`,
which pins the current behaviour even before the normative question is settled.
Coverage check to emit: `shm_kill_during_open_reservation`.

## Investigation log

### Q: In the shipped two-process topology, what is the "later producer" that the wedge blocks?

The catalog records no open question for this property. This is the question its
guarantee statement leaves implicit, and it decides what the test can assert.

- Sources examined: `ring.rs:1267-1340` (`try_reserve`), `:2271-2282`
  (`abort_reservation`), `:2488-2594` (all `abort_reservation` callers including
  `Drop`), `:2604-2612` (`DuplexRing::create`), `:1040-1091` (`create_in` and the
  random incarnation), `:1607-1745` (`conservation`);
  `crates/host-runtime/src/ring_transport.rs:749-800`, `:901-933` (the custody
  admission formerly at `shm_provider.rs:299-302` is gone; `ed487e11` replaced it
  with `admission.admit` at `ring_transport.rs:272-275`);
  `packages/shm-native/src/lib.rs:998-1079`, `:1082-1156`, `:1159-1210`;
  `crates/host-runtime/tests/support/shm_process.rs:256-292` (source tree; not at HEAD), `:644-757` (source tree; not at HEAD).
- Findings: the mechanism is confirmed exactly as the catalog states it — the
  derivation from `published + 1`, the losing CAS reported as `Exhausted`, and `Drop`
  as the sole restorer once a handle exists. What the record does not say is that a
  replacement producer on the same object does not arise in the shipped topology,
  because candidate preparation always creates a fresh `DuplexRing`. That does not
  invalidate the property; it relocates the observable consequence from "a later
  producer is blocked" to "the surviving side holds unreclaimable capacity that
  accounting reports as in-flight". The two-phase addon reservation
  (`lib.rs:1022-1029` and `:1133-1139`), which holds a claim across a return to
  JavaScript, is the widest real window and the right kill target.
- Missing evidence: none for the mechanism. What is untested rather than unknown is
  whether any deployment re-offers a descriptor whose object already carries a
  stranded reservation; the activation-token fence at
  `crates/host-runtime/tests/shm_failure_modes.rs:358` (source tree; not at HEAD) suggests re-offering is guarded at
  the negotiation layer, but that guard is about stale activation, not about slot
  state, so it is evidence of intent rather than of coverage.
- Conclusion: resolved with answer — the wedge is real and the mechanism is
  confirmed, but in the shipped topology it presents as permanent unreclaimable
  capacity on the surviving side rather than as a blocked replacement producer. The
  test therefore needs both arms above, and arm 1 requires a second producer on the
  same object, which is a same-process or re-attach arrangement rather than the
  ordinary two-process one.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 19, `:693-703` now `:1302-1311`: A losing CAS is no longer Exhausted: the map_err quarantines the ring and returns ProducerError::Ring(RingError::InvalidSharedState), which the comment at `:1300-1301` and the test foreign_slot_state_on_reserve_is_a_fault_not_backpressure (`:3960`) both pin as a fault rather than backpressure.
  - line 21, `ring.rs:679` now `ring.rs:1963`: At HEAD published is loaded Acquire inside verified_producer_cursors (`:1963`) and the whole cursor set must equal this handle's own record (`:1968-1970`), so a handle cannot adopt a cursor value it did not write itself.
  - line 58, `:105` now `:214`: Two kill-based tests remain at HEAD, setup_active_and_idle_sigkill_each_return_exact_capacity (`:214`) and repeated_crashes_do_not_ratchet_single_connection_capacity (`:226`), and both still kill outside a reservation.
  - line 70, `:693-703` now `:1302-1311`: A CAS that loses against a stranded PRODUCER_RESERVED slot quarantines the ring and returns InvalidSharedState, so at HEAD the wedge presents as a terminal fault rather than as Exhausted.
  - line 81, `:1209` now `:2368`: The cursor is published by Self::advance_cursor(&producer.published, ...) (`:2368`), an AcqRel compare_exchange against this handle's recorded value, not a plain release store.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 58, `:246` (kill-based failure test): The six-test kill matrix collapsed into the two tests at `:214` and `:226`; no third kill-based test remains.
  - line 58, `:282` (kill-based failure test): No fourth kill-based test remains in this file.
  - line 59, `:316` (kill-based failure test): No fifth kill-based test remains in this file.
  - line 59, `:358` (kill-based failure test): The file ends at line 350 and holds five tests, only two of which kill.
  - line 111, `crates/host-runtime/tests/support/shm_process.rs:257-263` (RoleProcess::kill): crates/host-runtime/tests/support/shm_process.rs no longer exists; the kill helper at HEAD is Victim::kill (`crates/host-runtime/tests/shm_failure_modes.rs:155-159`).
  - line 112, `:272-292` (reap_killed with its signal-9 assertion): The signal assertion now sits inside Victim::kill (`crates/host-runtime/tests/shm_failure_modes.rs:155-159`); no separate reaper exists.
  - line 113, `:266-269` (observation_window): No equivalent helper exists; the failure tests bound their waits with the BUDGET constant (`crates/host-runtime/tests/shm_failure_modes.rs:16`).
  - line 114, `:712-749` (the five victim scenarios): The scenario table is gone; crash_victim (`crates/host-runtime/tests/shm_failure_modes.rs:186-200`) drives the three roles setup, active, and idle.
  - line 143, `crates/host-runtime/tests/support/shm_process.rs:256-292` (the kill and reap helpers): The support module was removed; Victim (`crates/host-runtime/tests/shm_failure_modes.rs:116-169`) replaces it.
  - line 143, `:644-757` (the victim scenario table): No scenario table exists at HEAD; crash_victim (`crates/host-runtime/tests/shm_failure_modes.rs:186-200`) is the only driver.
  - line 157, `crates/host-runtime/tests/shm_failure_modes.rs:358` (the activation-token fence test): The file ends at line 350 and has no activation-token test; the nearest guard is daemon_restart_discards_old_rings_and_accepts_fresh_client (`:303`), which asserts a stale generation stops working and the successor takes a new daemon identity.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
