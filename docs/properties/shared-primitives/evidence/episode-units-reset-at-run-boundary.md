# `episode-units-reset-at-run-boundary`

- **Discovery:** cache-stability state-machine pass, durability classes.
- **Primary evidence:** `CoreState::step` (`crates/cache-stability/src/lib.rs:182-219`) retains only `DurabilityClass::Lineage` units in `frozen_units` and `pending_changes` when `run_started` is set, before dispatching to any action arm; `DurabilityClass` (`crates/cache-stability/src/lib.rs:59-64`) documents that every current cache unit is `Lineage` and `Episode` is reserved.
- **Existing evidence:** `run_started_keeps_lineage_resets_episode` (`crates/cache-stability/src/lib.rs:682-704`) mixes one unit of each class and asserts the survivor list and its bytes; `cross_episode_lineage_reproduces_byte_identical` (`crates/cache-stability/tests/golden_vectors.rs:274-322`) drives a lineage vector across a run boundary.
- **Failure scenario:** an inverted filter drops lineage units, so the prefix un-compacts at every new run; a missing filter keeps run-scoped units alive across runs.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The test's input contains both classes, so either inversion or omission changes the asserted key list.
- **Open-question log:** none.
