# `addon-reservations-drop-before-the-ring`

- **Discovery:** U3, when the native addon's channel registry was catalogued.
- **Primary evidence:** `Channel` (`packages/shm-native/src/lib.rs`) declares `producers`, `active`, and `stranded` before `to_host` and `from_host`, and the field comment states that the order is load-bearing because Rust drops fields in declaration order. `close_channel` detaches every producer, every active lease, and every stranded alias with `?`, so a detachment failure returns before the entry is removed; `close` then keeps the entry registered while `producers`, `active`, or `stranded` is non-empty and removes it otherwise.
- **Existing evidence:** `channel_drops_borrowing_reservations_before_the_ring` builds a `Channel` holding a reservation that borrows `to_host` and drops it; the test passes only if the reservation is destroyed before the ring. `packages/shm-native/tests/runtime.ts` asserts detachment under Bun and records `detachment_unavailable` under Node, which is a capability refusal rather than a proof.
- **Failure scenario:** a field reorder or a new borrowing field declared after the rings, so a finalizer touches unmapped memory.
- **Timing window:** none at close; the harm lands later, when the JavaScript finalizer runs.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass for the drop-order half. The external half (no attached `Uint8Array` after a successful `close`) holds under Bun only; the detachment-failure path is the designed quarantine, so the check is scoped to `close` returning `Ok`.
- **Open-question log:** none.
