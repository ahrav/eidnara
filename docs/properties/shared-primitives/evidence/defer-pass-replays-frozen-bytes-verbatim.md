# `defer-pass-replays-frozen-bytes-verbatim`

- **Discovery:** cache-stability state-machine pass over `CoreState::step` and its three arms.
- **Primary evidence:** `step_defer` (`crates/cache-stability/src/lib.rs:181-212`) reads `input.queued`, `input.run_started`, and `boundary_match`; it never reads `rendered_units` and never assigns to a frozen unit's `frozen_payload`, so `cached_prefix_bytes()` cannot change on a defer. `version` is incremented only in `step_soft` and `step_hard`.
- **Existing evidence:** `defer_does_not_mutate_frozen_bytes_or_render` (`crates/cache-stability/src/lib.rs:313-330`) compares the prefix and the version before and after a defer; `run_started_keeps_lineage_resets_episode` (`crates/cache-stability/src/lib.rs:568-589`) checks that a lineage payload survives a run boundary byte-identical; every golden vector asserts `cached_prefix_bytes()` after each pass (`crates/cache-stability/tests/golden_vectors.rs:168-261`).
- **Failure scenario:** a refactor that re-renders on defer, or that applies `queued` units into `frozen_units` directly, changes the prefix on a pass that the harness did not render for.
- **Timing window:** none; the property is per call.
- **Instrumentation:** none needed; the state is a value.
- **Audit verdict (U2): pass. The assertions compare the prefix string and the version number, neither of which derives from the action under test; a `step_defer` that pushed `queued` into `frozen_units` would fail the prefix comparison in `hard_drains_pending_changes_into_the_bust` and the golden vectors with queued units.
- **Open-question log:** none.
