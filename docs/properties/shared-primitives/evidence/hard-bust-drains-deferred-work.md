# `hard-bust-drains-deferred-work`

- **Discovery:** cache-stability state-machine pass, coordinator drain rule.
- **Primary evidence:** `step_hard` (`crates/cache-stability/src/lib.rs:250-277`) drains `pending_changes` after the rendered units, skipping any deferred copy of a key the bust re-rendered, before `apply_units`, assigns `boundary_id` when `new_boundary_id` is `Some`, and clears `reconcile_pending` only when the pass minted a boundary or the prior anchor is still present.
- **Existing evidence:** `hard_drains_pending_changes_into_the_bust` (`crates/cache-stability/src/lib.rs:426-459`) queues a drop through a defer, confirms it is absent from the prefix, then busts hard with a baseline that omits it and asserts the drain, the mint, the prefix, and the cleared flag; `hard_prefers_rendered_units_over_deferred_copies_of_the_same_key` (`crates/cache-stability/src/lib.rs:397-424`) queues a stale copy of `m0`, re-renders `m0` on the HARD, and asserts the rendered bytes are frozen; `hard_without_mint_on_absent_boundary_keeps_reconcile_pending` (`crates/cache-stability/src/lib.rs:690-725`) asserts that a `Hard` with no mint at an absent anchor leaves the flag set.
- **Failure scenario:** a `Hard` that freezes only its rendered units leaves the queued drop pending forever, so the rendered context and the recorded state diverge.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The test's rendered baseline deliberately omits the queued unit, so the presence of `[dropped 1]` in the prefix after the bust can only come from the drain.
- **Open-question log:** none.
