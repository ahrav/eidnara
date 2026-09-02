# `anchor-holds-while-reconcile-pending`

- **Discovery:** cache-stability state-machine pass, coverage-extending `Soft`.
- **Primary evidence:** `step_soft` (`crates/cache-stability/src/lib.rs:215-248`) advances `boundary_id` inside a let chain guarded by `boundary_match && !self.reconcile_pending`; `reconcile_pending` is not assigned in this arm.
- **Existing evidence:** `soft_does_not_advance_anchor_while_reconcile_pending` (`crates/cache-stability/src/lib.rs:568-605`) reverts, sends a coverage-extending `Soft`, and asserts the anchor holds and the next absent defer still reconciles; `coverage_extending_soft_advances_anchor_keeps_m0_frozen` (`crates/cache-stability/src/lib.rs:498-566`) asserts the positive case and that m0 bytes stay frozen.
- **Failure scenario:** an unguarded advance strands a stale m0 under a fresh anchor; the next defer clears the flag against the new anchor and the rematerialize never fires.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The negative test asserts both the unchanged anchor and that the following defer still flags reconcile, which is the observable consequence of a stranded m0.
- **Open-question log:** none.
