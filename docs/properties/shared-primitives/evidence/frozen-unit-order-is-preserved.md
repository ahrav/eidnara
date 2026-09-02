# `frozen-unit-order-is-preserved`

- **Discovery:** cache-stability state-machine pass, `apply_units`.
- **Primary evidence:** `apply_units` (`crates/cache-stability/src/lib.rs:285-297`) finds an existing key with `iter_mut().find` and overwrites in place, otherwise pushes; input order is the iteration order.
- **Existing evidence:** `soft_replaces_by_key_keeps_slot_appends_new` (`crates/cache-stability/src/lib.rs:490-525`) asserts the key sequence `["m0", "m1", "d1"]`, the untouched slot 0 payload, and the replaced slot 1 payload; the golden vectors compare `cached_prefix_bytes()`, whose value depends on order.
- **Failure scenario:** a map-backed frozen set or a remove-then-push replacement changes byte order and busts the provider cache on the next pass.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The test observes order through the public `frozen_units` field and the payload of each slot; a `HashMap`-backed replacement would fail the key-sequence assertion.
- **Open-question log:** none.
