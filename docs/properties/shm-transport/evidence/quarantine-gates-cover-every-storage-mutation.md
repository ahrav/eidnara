# quarantine-gates-cover-every-storage-mutation

## Discovery trigger

An enumeration lens: list every function in `Ring` that mutates shared storage
state, then check each one against the quarantine gate. Five functions gate.
Three that mutate slot state and publish descriptors do not.

## Evidence trail

- A repository-wide grep for `is_quarantined` over `crates/` and
  `packages/shm-native/src` returns exactly six hits in
  `crates/shm-transport/src/backend/ring.rs`: the definition at `:1927` and
  the five gates at `:1275` (`try_reserve`), `:1396` (`try_receive`), `:1529`
  (`release`), `:1608` (`conservation`), and `:1888` (`probe`). This is the
  complete gate set.
- `ring.rs:2347-2386` is `commit_reservation`. It has no gate. It writes the
  descriptor with `write_volatile` at `:2364`, stores `SLOT_PUBLISHED` at
  `:2365`, advances `arena_write` at `:2366`, and stores `published` with
  `Release` at `:2368`. Re-verified at post-#131 HEAD: the function spans
  `:2347-2386` (the earlier catalog-versus-code span disagreement is moot after
  the rewrite).
- `ring.rs:2271-2282` is `abort_reservation`. It has no gate. It zeroes
  `reservation_len` at `:2279` and stores `SLOT_FREE` at `:2280`, returning the
  descriptor slot and its arena bytes to the free pool. **Correction:** the
  catalog cites `1150-1165`; the function is `1154-1162`.
- `ring.rs:2388-2420` is `write_reservation`, also ungated. It copies caller
  bytes into the arena at `:2401-2418`.
- The reachable callers of `abort_reservation` are all ungated:
  `ProducerReservation::write` on a write failure (`ring.rs:2525`), `commit` on
  each of its three failure paths (`:2547`, `:2552`, and `:2563` inside the
  `Err` arm), `abort` (`:2575`), and `Drop` (`:2587-2593`). So an ungated slot
  release happens on the ordinary error and drop paths, not just on an exotic
  one.
- `ring.rs:1281` shows `try_reserve` calling `reclaim_completed()` after its gate
  at `:1275`. `reclaim_completed` (`:2070-2151`) is called from that one site
  only, so its ungated `SLOT_FREE` store at `:2140` is reached only through a
  gated entry point. That is the one mutation path the gate set does cover
  transitively.
- `packages/shm-native/src/lib.rs:290-312` is `cleanup_created_refs`. On a
  failed `detach_all` it calls `ring.enter_quarantine()` at `:301` and moves the
  references into `stranded` at `:302`, with the comment at `:297-300`: "A
  failed detach leaves JS views possibly attached to ring memory." This is the
  trigger where an ungated abort matters most, because the quarantine exists
  precisely because a JavaScript alias may still point into the arena.
- `quarantine_channel` in the addon
  (`packages/shm-native/src/lib.rs:415-424`) quarantines both directions at
  `:421-422` and then walks producers at `:396-398`, calling
  `detach_producer(...)?.abort()`. The `?` means a mid-walk failure leaves later
  producers registered, and each surviving `ProducerReservation` will still
  abort ungated on drop.

## Failure scenario

1. The producer holds an open `ProducerReservation` over sequence N. The slot is
   `SLOT_PRODUCER_RESERVED` and `reservation_len` is set (`ring.rs:1302-1326`).
2. The peer publishes a structurally invalid frame in the other direction, or
   the addon's alias detach fails. Either raises quarantine:
   `ring.rs:1401` on the receive-validation path, or
   `packages/shm-native/src/lib.rs:301` on the detach path.
3. The producer calls `commit(body_len)`. `commit` performs no quarantine check,
   and `commit_reservation` performs none either, so the descriptor is written at
   `ring.rs:2364` and `published` advances at `:2368`. A frame is now published
   into a direction whose storage is considered unrecyclable.
4. Alternatively the producer drops the reservation. `Drop` calls
   `abort_reservation` (`ring.rs:2587-2593`), which stores `SLOT_FREE` at
   `:2280`. The descriptor slot and its arena range return to the free pool, and
   the next `try_reserve` may hand those exact bytes to a new frame while the
   stranded JavaScript view still points at them.

## Timing windows and dependencies

The window is the lifetime of any outstanding `ProducerReservation`, which is
caller-controlled and unbounded. It depends on quarantine being raised by a
party other than the reservation holder, which is why the two reachable triggers
matter: the receiver side at `ring.rs:1401`, and the addon's alias cleanup at
`packages/shm-native/src/lib.rs:301` and `:308`. `Ring` is
`PhantomData<Rc<()>>` (`ring.rs:1033`) and so thread-confined, meaning a single
`Ring` handle cannot be quarantined by another thread through the same handle;
the cross-side trigger goes through the shared byte instead. This property
therefore depends on `quarantine-authority-survives-peer-writes`: if the flag
can be cleared, gating `commit` would not help.

## What a test must construct

Hold a live `ProducerReservation` from `try_reserve`, then set the flag out of
band with `ring.enter_quarantine()` on the same `Ring` (which is sufficient for
the gate question and avoids needing a second process), then assert two things.
First, `reservation.commit(len)` returns an error and the `published` cursor is
unchanged, observed through a second read of `conservation()` before quarantine
or through a direct read of `ProducerPage::published`. Second, in a separate
case, drop the reservation and assert no slot returns to `SLOT_FREE`. The second
assertion cannot use `conservation()` as its oracle, because `conservation()`
short-circuits on a quarantined ring at `ring.rs:1608-1619` and reports the whole
depth and arena as quarantined without reading any slot. The oracle must read
`DescriptorSlot::state` directly.

## Investigation log

### Q: Is "a reservation admitted before quarantine may still publish" the intended contract?

- Sources examined: `ring.rs:2347-2386` and `:2271-2282` for the absent gates,
  the five gate sites, `docs/shm-transport.md` (the two sentences quoted
  below, "Quarantine retains the exact charges and permanently prevents that
  record's storage from being reused" and "Quarantine retains charges instead of
  making uncertain storage reusable", were at `:79` and `:57` pre-#131; the
  trimmed post-#131 document no longer contains them — its surviving quarantine
  language is the accounting text at `:21` and `:92`), and the addon close
  ordering at `packages/shm-native/src/lib.rs:407-424`.
- Findings: both documented sentences are unconditional and are about storage
  reuse, which is exactly what the ungated `abort_reservation` performs. Nothing
  in the code carries a comment explaining the omission, and the gated and
  ungated functions sit in the same `impl` block. The addon's own close paths
  quarantine first at `:421-422` and only then walk producers, which suggests
  the author expected quarantine to be raised before producer teardown rather
  than during it.
- Missing evidence: no plan requirement, comment, or test states whether
  in-flight reservations are meant to survive quarantine.
- Conclusion: unresolved, needs the intended close ordering stated. The evidence
  establishes that publication and slot release both proceed after quarantine;
  it does not establish whether that was a decision or an oversight.

### Q: Is the commit path gated at HEAD? (added 2026-09-05)

- Checked: `ProducerReservation::commit` (`ring.rs:2541-2545`) checks `is_quarantined()`, aborts the reservation, and returns `ProducerError::Quarantined`. `commit_after_quarantine_is_refused_and_aborts` (`:3208`) holds a reservation, quarantines, and asserts `published` stays zero.
- Conclusion: yes. The record is refreshed to Exercised: yes.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 13, `:1381` now `:1927`: The grep no longer returns six hits or five gates: at HEAD `crates/shm-transport/src/backend/ring.rs` gates in `attach` (`:1141`), `arm_data_wait_guarded` (`:1202`), `armed_wait_holds` (`:1228`), `try_reserve` (`:1275`), `try_receive` (`:1396` and `:1404`), `wait_for_data` (`:1478`), `release` (`:1529`), `conservation` (`:1608`), `probe` (`:1888`), `trim` (`:2248`), `publish_commit` (`:2382`), and `ProducerReservation::commit` (`:2541`), and `is_quarantined` is also read in `crates/host-runtime/src/ring_transport.rs:353` and `:926` and `packages/shm-native/src/lib.rs:1488`.
  - line 17, `ring.rs:1577-1627` now `ring.rs:2347-2386`: The commit path is gated at HEAD: `ProducerReservation::commit` checks `is_quarantined()`, aborts the reservation, and returns `ProducerError::Quarantined` (`:2541-2545`), and `publish_commit` re-checks quarantine after publication (`:2382`).
  - line 20, `:1620` now `:2368`: `published` now advances through an `advance_cursor` compare-exchange with `AcqRel`, not a plain `Release` store.
  - line 23, `ring.rs:1567-1575` now `ring.rs:2271-2282`: `abort_reservation` also punches the pages the reservation dirtied above `arena_write` and quarantines if that removal fails (`:2272-2277`).
  - line 30, `ring.rs:1760` now `ring.rs:2525`: These callers are no longer all ungated: `commit` checks `is_quarantined()` first and aborts (`:2541-2545`), so it has four abort paths and the first one is the gate.
  - line 36, `:1470-1566` now `:2070-2151`: `reclaim_completed` has two call sites at HEAD, `try_reserve` (`:1281`) and `trim` (`:2256`); both are gated, `trim` at `:2248`.
  - line 48, `:383-386` now `:396-398`: `quarantine_channel` now delegates to `detach_all_aliases`, which records the first failure and finishes the sweep instead of propagating with `?`, so a mid-walk failure no longer leaves later producers registered.
  - line 59, `ring.rs:1098` now `ring.rs:1401`: Quarantine on the receive path is no longer raised inside the validation arm; `try_receive` maps every `try_receive_inner` error through `quarantine_with`.
  - line 63, `ring.rs:1617` now `ring.rs:2364`: This step is refused at HEAD: `commit` checks `is_quarantined()` and aborts the reservation before `prepare_commit` runs (`:2541-2545`), so a reservation admitted before quarantine cannot publish.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
