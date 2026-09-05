# `cache-stability-golden-vectors-are-byte-stable`

- **Discovery:** golden-vector pass at U2; the fixture is the cross-harness contract.
- **Primary evidence:** `crates/cache-stability/tests/golden/cache-stability-golden-vectors.json` is authored in this tree and tracked byte for byte. Changing the file is a reviewed contract change under R18; the change shows as a diff to the fixture.
- **Existing evidence:** `golden_fixture_is_schema_v3_with_eleven_vectors` (`crates/cache-stability/tests/golden_vectors.rs:96-105`), `core_state_schema_v3_empty_wire_format_is_stable` (`crates/cache-stability/tests/golden_vectors.rs:107-157`), `all_golden_vectors_pass` (`crates/cache-stability/tests/golden_vectors.rs:159-166`), `cross_episode_lineage_reproduces_byte_identical` (`crates/cache-stability/tests/golden_vectors.rs:274-322`); all pass on Rust 1.98 and stable in this workspace and reproduce every vector from the in-tree core.
- **Failure scenario:** a regenerated fixture passes its own test; only the diff to the tracked fixture shows a changed contract.
- **Timing window:** none.
- **Instrumentation:** none beyond the tracked fixture; review of a fixture diff is the oracle for byte stability.
- **Audit verdict (U2): pass. Independent oracle with one noted limit: `run_vector` (`crates/cache-stability/tests/golden_vectors.rs:168-272`) passes `expect_action` as the `proposed` action, so the action-equality assertion is near-tautological. The assertions on `cached_prefix_bytes()`, `boundary_id`, `reconcile_pending`, and the pending-change count are independent of the input and carry the byte-stability claim; the in-crate mechanism tests cover the classifier-independent arms.
- **Open-question log:** none.
