# `hard-bust-drains-deferred-work`

- **Discovery:** cache-stability state-machine pass, coordinator drain rule.
- **Primary evidence:** `step_hard` (`crates/cache-stability/src/lib.rs:253-265`) moves `pending_changes` into the rendered set with `Vec::append` before `apply_units`, assigns `boundary_id` when `new_boundary_id` is `Some`, and clears `reconcile_pending` only when the pass minted a boundary or the prior anchor is still present.
- **Existing evidence:** `hard_drains_pending_changes_into_the_bust` (`crates/cache-stability/src/lib.rs:390-423`) queues a drop through a defer, confirms it is absent from the prefix, then busts hard with a baseline that omits it and asserts the drain, the mint, the prefix, and the cleared flag; `hard_without_mint_on_absent_boundary_keeps_reconcile_pending` (`crates/cache-stability/src/lib.rs:654-689`) asserts that a `Hard` with no mint at an absent anchor leaves the flag set.
- **Failure scenario:** a `Hard` that freezes only its rendered units leaves the queued drop pending forever, so the rendered context and the recorded state diverge.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The test's rendered baseline deliberately omits the queued unit, so the presence of `[dropped 1]` in the prefix after the bust can only come from the drain.
- **Open-question log:** none.
