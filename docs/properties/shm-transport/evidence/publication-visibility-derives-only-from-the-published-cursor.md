# publication-visibility-derives-only-from-the-published-cursor

## Discovery trigger

A memory-ordering lens over every access to `DescriptorSlot::state`: enumerate the
stores and loads, note each one's `Ordering`, and check which loads are gated by
an acquire on a publication cursor. One store is `Relaxed` where its five
siblings are `Release`, and one load has no gate.

## Evidence trail

- A grep for `.state.load`, `.state.store`, and `.state.compare_exchange` in
  `crates/shm-transport/src/backend/ring.rs` (non-test code) gives the complete
  access set at HEAD, which makes this analysis exhaustive rather than sampled:
  - stores: `:1315` and `:1319` (`try_reserve` rollback, `Release`), `:1452`
    (`try_receive` to `SLOT_RECEIVER_LEASED`, `Release`), `:2140`
    (`reclaim_completed` to `SLOT_FREE`, `Release`), `:2280`
    (`abort_reservation` to `SLOT_FREE`, `Release`), and `:2365`
    (`publish_commit` to `SLOT_PUBLISHED`, **`Relaxed`**);
  - compare-exchanges: `:1303` (`try_reserve`), `:1432` (`try_receive_inner`, to
    the intermediate `SLOT_RECEIVER_HELD`), `:1575` (`release`), all `AcqRel` on
    success and `Acquire` on failure;
  - plain loads: `:1681` (`conservation`), `:1836` and `:1869`
    (`validate_idle_window`), and `:2093` (`reclaim_completed`), all `Acquire`.
- `ring.rs:2364-2369` is the publication sequence, in program order:
  `write_volatile` of the descriptor at `:2364`, `state.store(SLOT_PUBLISHED,
  Relaxed)` at `:2365`, `arena_write.store(..., Relaxed)` at `:2366`, and
  `published.store(sequence, Release)` at `:2368` (re-verified at post-#131
  HEAD: `:1615` (source tree; not at HEAD) is the SAFETY comment, `:1616` (source tree; not at HEAD) the `unsafe {`, and the stores
  are `:2364-2369`).
- The consequence is precise. A `Relaxed` store does not head a release sequence,
  so the `Acquire` load of `state` at `:1681` has nothing to synchronize with when
  it observes `SLOT_PUBLISHED`. The `Release` store at `:2368` is to a different
  location (`ProducerPage::published`), so acquiring `state` does not order
  against it either. An observer that reaches `SLOT_PUBLISHED` through `:1681` has
  no happens-before edge to the descriptor write at `:2364`.
- The correct pattern is present in the same file, which is what makes the
  omission look unintentional. `reclaim_completed` loads
  `completion_sequence` with `Acquire` at `:2090` and only then loads `state` at
  `:2093` and reads the descriptor at `:2096`. The comment at `:2089` states the
  pairing: "SAFETY: acquire pairs with receiver release publication." The matching
  `Release` store is at `:1590-1591` in `release`.
- The receive path is also correctly gated. `try_receive` loads `published` with
  `Acquire` at `:1978`, checks `consumed == published` at `:1424-1426`, and only then
  compare-exchanges the slot at `:1431-1438` and reads the descriptor at `:1440`. The
  comment at `:1439` states the dependency: "SAFETY: acquire publication made
  descriptor visible; one read snapshots all fields."
- The ungated load's blast radius is bounded today. `conservation()`
  (`ring.rs:1607-1744`) reads only `state` at `:1681` and `reservation_len` at `:1683`
  and never touches `(*slot).descriptor` or arena bytes. So the present
  consequence is an accounting-accuracy question, not undefined behaviour.
- `ring.rs:150` shows `reservation_len` is `AtomicU64`, so the `Relaxed` load at
  `:1683` is a well-defined atomic read of a possibly stale value rather than a
  race.
- Existing check: none. `two_process_zero_copy_exchange_uses_authenticated_grant`
  (`crates/shm-transport/tests/ring.rs:489`) is the only cross-process test; it
  exchanges one frame in lockstep with a sleep, so it cannot place a reader inside
  the window.
- `docs/properties/shm-transport/existing-checks.md:393-395` records that
  no loom, shuttle, Miri, or ThreadSanitizer configuration exists anywhere in the
  repository, so no tool currently checks any ordering choice in this file.
  At HEAD: The comment now reads "The acquire exchange above pairs with the producer's release of `published`", and the one-read snapshot it referred to is now `DescriptorSlot::read_descriptor` (`:194-199`).
  At HEAD: The load moved into `verified_published` (`:1976-1986`), which `try_receive_inner` calls at `:1423`; that helper also rejects a rewound or over-far `published`.
  At HEAD: The comment at `:2089` reads "Acquire pairs with the receiver's release store of `completion_sequence`", not "SAFETY: acquire pairs with receiver release publication"; the pairing it names is unchanged.
  At HEAD: The store to `ProducerPage::published` is now an AcqRel compare_exchange inside `Self::advance_cursor`, so the argument holds a fortiori: it is still a different location from `state`.
  At HEAD: `published` is no longer a Release store either; it moves through the same AcqRel compare_exchange in `Self::advance_cursor` (`:1951-1956`).
  At HEAD: `arena_write` is no longer a Relaxed store: `Self::advance_cursor` (`:1951-1956`) moves it with an AcqRel compare_exchange that fails closed if the peer rewrote the cursor.
  At HEAD: the state access set is larger than the six stores, three exchanges, and two loads listed here: `validate_idle_window` adds two more Acquire loads of `state` (`:1836` and `:1869`), and the exchange in `try_receive_inner` now moves the slot to the intermediate `SLOT_RECEIVER_HELD`.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. The producer commits. The descriptor bytes are written at `ring.rs:2364` and
   the slot state becomes `SLOT_PUBLISHED` at `:2365` with `Relaxed` ordering.
2. On a weakly-ordered target the store at `:2365` may become visible to another
   core before the descriptor write at `:2364`, because no barrier separates them
   and the store carries no release semantics.
3. An observer in the peer process calls `conservation()`, loads `state` at
   `:1681`, and observes `SLOT_PUBLISHED`.
4. Today it stops there and only mis-attributes bytes. The unsoundness is
   conditional on a future reader: any code that follows this path into
   `(*slot).descriptor` or the arena would read bytes it has no ordering edge to,
   which is a data race and undefined behaviour rather than a stale value, because
   `descriptor` is an `UnsafeCell<SharedDescriptor>` (`ring.rs:151`) accessed with
   `read_volatile` and not an atomic.

The reason this is cataloged as safety rather than as accounting is step 4. The
guard against it is a convention that nothing reads the descriptor from slot
state, and no comment, type, or test enforces that convention.

## Timing windows and dependencies

The window is the reorder distance between the two stores at `:2364` and `:2365`,
which is a hardware and compiler property with no upper bound in the abstract
machine. On x86-64's TSO model, store-store reordering is not permitted, so the
window is empirically unobservable there even though the Rust abstract machine
permits it; on aarch64 or Graviton it is observable. That makes platform gating
essential to any test: a passing result on x86-64 proves nothing. The property
depends on `reservation-charge-visible-with-non-free-state` for its practical
severity, because both are only observable through `conservation()`, and that
function has no production caller at this commit (`Ring::probe` at `ring.rs:1887`
is its only non-test caller, and `ShmRecoveryBackend::probe` at
the host-side readiness probe that returned a constant without touching a
`Ring`).
At HEAD: The conservation walk does have a production caller at HEAD: `Ring::attach` runs `conservation_inner(true)` at `:1148` to refuse a mapping the peer already broke, so only the counts-returning `conservation()` remains probe-only.

## What a test must construct

Either a genuine cross-process race on a weakly-ordered target, or a model
checker. The concrete shape for the hardware route is: one process committing
frames in a tight loop while a second process polls `conservation()` on the same
mapping, on aarch64, with an oracle that is not `ArenaCounts::conserves` because
that predicate is arithmetically self-satisfying (`arena.rs:208-222` against
`ring.rs:1739-1743`). The oracle must be a per-slot cross-check of `reservation_len`
and the descriptor's `allocation_len` for slots the observer finds non-free.

The model-checker route is more tractable and does not need the hardware. Extract
the slot state machine and the four cursors into a `cfg(loom)` harness with two
threads, one running the commit sequence of `:2364-2369` and one running the
observer sequence of `:1679-1683` extended to read the descriptor, and assert the
observer never sees a `SLOT_PUBLISHED` slot with an unwritten descriptor. That
extension is the point: it asserts the property the current code relies on
convention for. Neither route exists today, and adding loom is itself the missing
capability recorded as F5 in
`docs/properties/part-1-shm-transport/fault-map.md`.

## Investigation log

### Q: Is the relaxed state store intentional, given `abort_reservation` and `reclaim_completed` use `Release` for the same field?

- Sources examined: the complete `state` access grep listed above;
  `ring.rs:2364-2369` (the publication sequence and its SAFETY comment at
  `:1615` (source tree; not at HEAD), "producer exclusively owns reserved slot and arena range");
  `:2135-2141` (`reclaim_completed`'s stores; the pre-#131 comment "producer
  alone reclaims in publication order" now reads "removal succeeded and producer
  exclusively publishes reclaimed capacity" at `:2142`); `:2271-2282`
  (`abort_reservation`, comment at `:1569` (source tree; not at HEAD), "reservation owner calls only before
  publication"); `:1312-1324` (the rollback stores at `:1315` and `:1319`, whose
  comments at `:952` (source tree; not at HEAD) and `:957` (source tree; not at HEAD) cite producer ownership); and `:2089` and `:1439`
  for the two comments that do name acquire-release pairings.
- Findings: the file's comments are consistent about *ownership* and silent about
  *ordering* for the state field specifically. The only two comments that name a
  pairing are on the `published` and `completion_sequence` cursors, which matches
  the property's own statement that those are the two intended publication edges.
  Read that way, `Relaxed` at `:2365` is defensible: state is meant to be a slot
  ownership token, not a publication edge, and the `Release` on the other five
  stores is then incidental rather than load-bearing. Nothing states this, and
  the SAFETY comment at `:1682` in `conservation` cuts the other way by asserting
  an ordering guarantee about a sibling field.
- Missing evidence: no comment, commit message, or plan requirement addresses why
  one store of six is `Relaxed`. There is also no tool result to appeal to, since
  no concurrency checker is configured anywhere in the repository.
- Conclusion: unresolved, needs the author's reasoning recorded in the code. The
  mechanism is fully established and the reading above is a hypothesis, not a
  finding; asserting it as the intent would be fabrication. What is settled
  without an answer: the `Acquire` load at `:1681` synchronizes with nothing when it
  observes `SLOT_PUBLISHED`, and the invariant that keeps that safe is unwritten.
  At HEAD: The comment is no longer a SAFETY comment and reads "The reservation length is assigned before any non-free state becomes visible"; the load below it needs no unsafe block now.
  At HEAD: The comment at `:2142` reads "Capacity becomes visible only after every removal succeeded"; it no longer mentions producer-exclusive publication, and it now sits above the two `advance_cursor` calls rather than the slot stores.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 22, `:1271` now `:1681`: At HEAD the state access set is larger than the six stores, three exchanges, and two loads listed here: `validate_idle_window` adds two more Acquire loads of `state` (`:1836` and `:1869`), and the exchange in `try_receive_inner` now moves the slot to the intermediate `SLOT_RECEIVER_HELD`.
  - line 26, `:1619` now `:2366`: `arena_write` is no longer a Relaxed store: `Self::advance_cursor` (`:1951-1956`) moves it with an AcqRel compare_exchange that fails closed if the peer rewrote the cursor.
  - line 27, `:1620` now `:2368`: `published` is no longer a Release store either; it moves through the same AcqRel compare_exchange in `Self::advance_cursor` (`:1951-1956`).
  - line 32, `:1620` now `:2368`: The store to `ProducerPage::published` is now an AcqRel compare_exchange inside `Self::advance_cursor`, so the argument holds a fortiori: it is still a different location from `state`.
  - line 39, `:1481` now `:2089`: The comment at `:2089` reads "Acquire pairs with the receiver's release store of `completion_sequence`", not "SAFETY: acquire pairs with receiver release publication"; the pairing it names is unchanged.
  - line 43, `:1072` now `:1978`: The load moved into `verified_published` (`:1976-1986`), which `try_receive_inner` calls at `:1423`; that helper also rejects a rewound or over-far `published`.
  - line 45, `:1092` now `:1439`: The comment now reads "The acquire exchange above pairs with the producer's release of `published`", and the one-read snapshot it referred to is now `DescriptorSlot::read_descriptor` (`:194-199`).
  - line 92, `ring.rs:1336` now `ring.rs:1887`: The conservation walk does have a production caller at HEAD: `Ring::attach` runs `conservation_inner(true)` at `:1148` to refuse a mapping the peer already broke, so only the counts-returning `conservation()` remains probe-only.
  - line 126, `:1550` now `:2142`: The comment at `:2142` reads "Capacity becomes visible only after every removal succeeded"; it no longer mentions producer-exclusive publication, and it now sits above the two `advance_cursor` calls rather than the slot stores.
  - line 138, `:1270` now `:1682`: The comment is no longer a SAFETY comment and reads "The reservation length is assigned before any non-free state becomes visible"; the load below it needs no unsafe block now.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 28, `:1615` (SAFETY comment above the publication sequence): No SAFETY comment guards the publish path at HEAD; the only prose there is the doc comment on `publish_commit` (`:2345-2346`).
  - line 28, `:1616` (`unsafe {` opening the publication sequence): `publish_commit` contains no unsafe block; the volatile write is encapsulated in `DescriptorSlot::write_descriptor` (`:201-204`).
  - line 123, `:1615` (SAFETY comment "producer exclusively owns reserved slot and arena range"): No such comment exists at HEAD; the publish path carries only the doc comment at `:2345-2346`.
  - line 127, `:1569` (comment "reservation owner calls only before publication"): Gone; `abort_reservation`'s doc comment (`:2268-2270`) now explains page removal above `arena_write` instead of ownership.
  - line 129, `:952` (comment citing producer ownership above the Exhausted rollback): The Exhausted arm carries no comment at HEAD; the only nearby comment (`:1300-1301`) explains why a non-free slot is corruption.
  - line 129, `:957` (comment citing producer ownership above the fault rollback): Replaced by "Cursors the protocol cannot produce are a fault, not backpressure" (`:1320`), which says nothing about ownership.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
