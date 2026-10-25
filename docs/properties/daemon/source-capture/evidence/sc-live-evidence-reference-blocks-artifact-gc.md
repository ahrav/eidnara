# sc-live-evidence-reference-blocks-artifact-gc

## Discovery trigger

R11 and C11 state physical retention: captured bytes must not outlive their
deadline forever, and must not be reclaimed while referenced. Before U4 the
publisher stored harness evidence `canonical` with no deadline, so the only
thing keeping bytes was this GC rule. Part A asked which test drives a
`canonical` row through a far-future sweep.

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/cas/gc.rs`: `REFERENCED_GRACE_MS = 14 days`;
  `ORPHAN_GRACE_MS = 1 hour`.
- `prepare_reclaim`, in order:
  1. a digest in `artifact_pending_unlinks` returns `None` (proceed); this
     check precedes the live-reference check, so a purge unlinks bytes a
     live row still names;
  2. any live `evidence_meta` row with the digest returns
     `Withheld::Retained`;
  3. any pin ref whose `COALESCE(r.released_at, p.released_at)` is null
     returns `Retained` (expiry and purge degradation are not consulted
     here);
  4. a `Reclaiming` reservation defers to the replay check;
  5. a current-epoch `Live` reservation with an unexpired lease retains;
  6. with an invalidation or pin release, the deadline is the maximum of
     `MAX(retain_until)`, `invalidated_at + 14 d`, and
     `pin_released_at + 14 d`; `now < deadline` retains; an overflow
     retains;
  7. otherwise the orphan mtime grace applies.
- `run_artifact_gc` / `reclaim_candidate`: unlinks under a fenced write
  transaction and deletes the digest's pending-unlink row. It inserts no
  purge tombstone.
- `crates/kernel/src/local_file.rs`, `expire_captures`: invalidates rows
  only; bytes are left for GC.

Tests read at HEAD (`crates/kernel/tests/kernel_gc.rs`):
`referenced_and_recently_invalidated_artifacts_survive` (a live `canonical`,
`retain_until: None` row and a recently invalidated row survive
`run_staging_maintenance(13 * DAY_MS)`),
`invalidated_artifact_reclaims_after_grace_but_backward_clock_keeps_it`,
`retain_until_is_an_absolute_floor_not_an_extra_grace_start`,
`overflowing_grace_deadline_keeps_artifact` (`now = i64::MAX` on an
invalidated row), `active_capture_pin_and_pin_release_grace_protect_artifact`,
`maintenance_preserves_only_the_grace_remaining_after_pin_expiry`,
`pending_purge_unlinks_a_live_reference_and_sweeps_its_digest_temps`.
`crates/daemon/tests/harness_sources.rs`,
`harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it`
keeps `canonical` rows live through `expire_harness_captures(NOW + 32 d)`; it
drives the expiry sweep, not GC.

## Failure scenario

GC reclaims bytes a live descriptor cites, so a later read of exact source
bytes fails; or GC never reclaims invalidated capture bytes, so expiry
bounds rows but not bytes.

## Timing windows and dependencies

Physical retention after expiry is at least `retain_until` plus the 14-day
grace (C11). A `Reclaiming` reservation carries the decision across a crash.
The live-reference check precedes every clock comparison, so a live row is
retained at any `now`.

## What a test must construct

F15: a live `canonical` row with `retain_until: None` through
`run_staging_maintenance` at a far-future `now`; invalidated evidence inside
and beyond 14 days; a held pin; and a pending unlink over a live row (the
purge carve-out).

## Investigation log

### Q: Which existing GC test drives a `canonical` row through a far-future sweep?
- Sources examined: every test in `kernel_gc.rs`; the U4 harness test.
- Findings: `referenced_and_recently_invalidated_artifacts_survive` keeps a
  live `canonical`/`None` row through a GC sweep, but at
  `now = 13 * DAY_MS` after the epoch, which is earlier than the wall-clock
  ingest. `overflowing_grace_deadline_keeps_artifact` uses `i64::MAX` only
  for an invalidated row. The U4 test's `NOW + 32 d` sweep is the expiry
  sweep. By code reading the live-reference arm ignores `now`.
- Missing evidence: a GC sweep at a far-future `now` over a live row.
- Conclusion: unresolved, needs one `kernel_gc` test at a far-future `now`;
  the code-reading answer is that no `now` reclaims a live row.

### Q: Does the guarantee hold under a pending purge?
- Findings: no; `prepare_reclaim` returns `None` for a pending unlink before
  reading live references, and
  `pending_purge_unlinks_a_live_reference_and_sweeps_its_digest_temps`
  asserts the unlink. An operator purge is the intended override.
- Conclusion: resolved with answer; the guarantee carves out purges.
