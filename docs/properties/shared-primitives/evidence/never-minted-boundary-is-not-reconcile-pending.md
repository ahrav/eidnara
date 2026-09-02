# `never-minted-boundary-is-not-reconcile-pending`

- **Discovery:** cache-stability state-machine pass, vacuous-boundary guard.
- **Primary evidence:** `step_defer` sets `self.reconcile_pending = !boundary_match && !self.boundary_id.is_empty()` (`crates/cache-stability/src/lib.rs:206`); the comment block above it records the oscillation the guard prevents.
- **Existing evidence:** `defer_on_never_minted_boundary_is_stable_not_reconcile_pending` (`crates/cache-stability/src/lib.rs:334-365`) runs three defers against an empty id and asserts the flag stays clear, then mints `b1` and asserts an absent-boundary defer does set it; `defer_boundary_absent_keeps_bytes_and_sets_reconcile_pending` (`crates/cache-stability/src/lib.rs:369-383`) covers the non-vacuous arm alone.
- **Failure scenario:** without the `is_empty` term every fresh store alternates `Hard` and defer; with a wrong term a real revert is never reconciled.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. Both truth-table rows that differ between the guarded and unguarded expression are asserted.
- **Open-question log:** none.
