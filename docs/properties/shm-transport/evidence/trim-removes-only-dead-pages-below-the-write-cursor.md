# trim-removes-only-dead-pages-below-the-write-cursor

## Discovery trigger

Review of the reclamation records found that
`reclamation-excludes-pages-with-live-wrapped-bytes` covers the reclaim-time
page-removal pass only, while `Ring::trim`
(`crates/shm-transport/src/backend/ring.rs:2259`) is a second entry into the same
`punch_dead_pages` with three dedicated tests and no record.

## Evidence trail

- `ring.rs:2259-2278` is `pub fn trim(&self) -> Result<(), RingError>`. It returns
  `RingError::Quarantined` when `is_quarantined()` holds, `RingError::RoleMismatch`
  when `self.producer` is false, calls `self.reclaim_completed()?`, reads
  `arena_write` and `arena_reclaimed` from `verified_producer_cursors()`, and calls
  `self.punch_dead_pages(arena_reclaimed, arena_write, true)`. Both fallible
  reads map their error through `quarantine_with`.
- `punch_dead_pages` (`:2172-2260`): the dead range is `[punched, reclaimed)`
  (`:2172`); `everything` counts partially covered pages as dead once no live
  bytes remain (`:2173`, `:2235-2238`); the function returns early when
  `punched == reclaimed` (`:2185-2187`), so a ring with nothing released
  removes nothing. `live_end` (`:2163-2165`) returns the producer-local
  `reserved_end` when a reservation is open, which is what keeps an uncommitted
  reservation's page out of the dead range; the shared `arena_write` cursor
  excludes that reservation (`:2256-2258`), which is why `trim` is producer-only.
- `quarantine_survives_peer_clearing_shared_flag` (`:4257`) asserts
  `ring.trim()` returns `Err(RingError::Quarantined)` at `:4270`; the
  syscall-counter test asserts `page_removals == 1` after one `trim`
  (`:3004-3005`).
- The comment at `:2266-2267` states the reason for the reclaim call: releases
  become reclaimed capacity only through that pass, which otherwise runs inside
  `try_reserve`, so an idle ring would keep newly dead pages resident without it.
- `reclaim_completed` is at `:2073`; `punch_dead_pages` at `:2178`;
  `resident_arena_pages` at `:1857`; `quarantine_with` at `:1920`.
- `only_a_producer_handle_may_trim` (`:3150`) asserts the consumer handle's `trim`
  returns `Err(RingError::RoleMismatch)` and the producer's succeeds.
- `trim_reclaims_pending_releases_before_punching` (`:3276`) publishes and
  releases so that `resident_arena_pages()` is 2, calls `trim`, and asserts the
  residency drops with the message "an idle trim must reclaim the released frame
  before punching"; it also asserts descriptor and byte capacity return to the
  grant totals.
- `trim_preserves_bytes_of_an_uncommitted_reservation` (`:4183`) takes a
  reservation whose start lies inside the page `trim` would otherwise treat as
  fully dead (comment at `:4191`), calls `trim`, and asserts the reservation's
  bytes survive.
- Caller search: `.trim()` on a ring appears only in `ring.rs` tests (`:3004`,
  `:3155`, `:3159`, `:3282`) and in the `:4183` test. `crates/host-runtime/src` and
  `packages/shm-native/src` have no `Ring::trim` call; the two `.trim()` hits in
  `crates/shm-transport/src/profile.rs:273` and `:278` are string trims.

## Failure scenario

A trim whose upper bound is the reclaim cursor's target rather than
`arena_write`, or whose trailing-page handling ignores a reservation that
starts inside the last dead page, punches a page the producer is writing into.
The receiver later decodes zeros as a valid frame body. A missing role gate lets
the consumer side punch under the producer's reservation with the same effect.

## Timing windows and dependencies

No cross-thread window: `trim` runs on the producer handle and reads the
producer's own cursors after the reclaim pass. The dependency is the ordering
reclaim-then-punch, and the bound `arena_write`.

## What a test must construct

An idle ring with one released frame and no reservation in flight (to show the
reclaim call inside `trim` is what frees the page); a reservation taken but not
committed whose start lies inside the trailing dead page; a consumer handle on
the same ring. Assert residency, byte survival, and the role error.

## Investigation log

### Q: Is `Ring::trim` on any shipped path?

- Sources examined: `crates/host-runtime/src`, `packages/shm-native/src`,
  `crates/shm-transport/src` (ripgrep for `.trim()`).
- Findings: only in-crate tests call it; the other hits are `str::trim`.
- Missing evidence: none.
- Conclusion: `test-only`; the record is a regression contract for a future
  idle-connection caller.

### Q: What bounds the punch?

- Sources examined: `ring.rs:2259-2278`, `:2178`, `:4183-4200`.
- Findings: `punch_dead_pages(arena_reclaimed, arena_write, true)`; the third
  argument enables trailing-page removal; the `:4183` test pins that a
  reservation above `arena_write` survives.
- Missing evidence: the exact trailing-page rule inside `punch_dead_pages` was
  not re-derived here; it is covered by the reclamation record's `removal_ranges`
  analysis.
- Conclusion: the guarantee is stated against the dead range
  `[punched, arena_reclaimed)` with partial pages removable under `everything`;
  the protective bound for reserved bytes is `live_end()`'s `reserved_end`, not
  `arena_write`, and the Check requires a released frame so the removal path is
  actually taken.
