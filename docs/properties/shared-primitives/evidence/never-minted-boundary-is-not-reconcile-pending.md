# `never-minted-boundary-is-not-reconcile-pending`

- **Discovery:** cache-stability state-machine pass, vacuous-boundary guard.
- **Primary evidence:** `step_defer` sets `self.reconcile_pending = !boundary_match && !self.boundary_id.is_empty()` (`crates/cache-stability/src/lib.rs:207`); the comment block above it records the oscillation the guard prevents.
- **Existing evidence:** `defer_on_never_minted_boundary_is_stable_not_reconcile_pending` (`crates/cache-stability/src/lib.rs:337-370`) runs three defers against an empty id and asserts the flag stays clear, then mints `b1` and asserts an absent-boundary defer does set it; `defer_boundary_absent_keeps_bytes_and_sets_reconcile_pending` (`crates/cache-stability/src/lib.rs:372-388`) covers the non-vacuous arm alone.
- **Failure scenario:** without the `is_empty` term every fresh store alternates `Hard` and defer; with a wrong term a real revert is never reconciled.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The guarded expression differs from the unguarded `!boundary_match` in exactly one row of the truth table: `boundary_match == false` with an empty `boundary_id`, where the guard yields `false` and the unguarded form yields `true`. That row is asserted three times by the empty-id defers. Of the three rows where the two forms agree, the tests also assert `boundary_match == false` with a non-empty id (flag set, in both tests) and `boundary_match == true` with a non-empty id (flag clear, in `defer_does_not_mutate_frozen_bytes_or_render`); `boundary_match == true` with an empty id, a defer whose `boundary_present` is itself `""`, is not exercised.
- **Open-question log:** none.
