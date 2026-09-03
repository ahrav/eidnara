# `anchor-holds-while-reconcile-pending`

- **Discovery:** cache-stability state-machine pass, coverage-extending `Soft`.
- **Primary evidence:** `step_soft` sets `reconcile_pending` when `boundary_match` is false and the boundary is not the never-minted sentinel, before the guarded anchor advance, so a `Soft` that is the first pass to find the anchor gone reports the stale baseline on that pass (`soft_with_an_absent_anchor_marks_reconcile_pending`).  `step_soft` (`crates/cache-stability/src/lib.rs:249-292`) advances `boundary_id` inside a let chain guarded by `boundary_match && !self.reconcile_pending`; `reconcile_pending` is not assigned in this arm.
- **Existing evidence:** `soft_does_not_advance_anchor_while_reconcile_pending` (`crates/cache-stability/src/lib.rs:649-690`) reverts, sends a coverage-extending `Soft`, and asserts the anchor holds and the next absent defer still reconciles; `coverage_extending_soft_advances_anchor_keeps_m0_frozen` (`crates/cache-stability/src/lib.rs:575-647`) asserts the positive case and that m0 bytes stay frozen.
- **Failure scenario:** an unguarded advance strands a stale m0 under a fresh anchor; the next defer clears the flag against the new anchor and the rematerialize never fires.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The negative test asserts both the unchanged anchor and that the following defer still flags reconcile, which is the observable consequence of a stranded m0.
- **Open-question log:** none.
