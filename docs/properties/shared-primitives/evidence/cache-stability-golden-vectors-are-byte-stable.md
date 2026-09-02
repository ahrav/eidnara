# `cache-stability-golden-vectors-are-byte-stable`

- **Discovery:** golden-vector pass at U2; the fixture is the cross-harness contract.
- **Primary evidence:** destination `crates/cache-stability/tests/golden/cache-stability-golden-vectors.json` sha256 `3a3b65fab99cd50c81ac2a1956e14652d1a635b091f6f7841229065800c5c11e`; source blob `67b5ec9fabce080bf1d37b35df41fe20d9dd5565` at `commons@89abb40` has the same content sha256. The receipt entry is `verbatim`, and the registry lists the file as a `byte-stable` fixture.
- **Existing evidence:** `golden_fixture_is_schema_v3_with_eleven_vectors` (`crates/cache-stability/tests/golden_vectors.rs:97`), `core_state_schema_v3_empty_wire_format_is_stable` (`crates/cache-stability/tests/golden_vectors.rs:108`), `all_golden_vectors_pass` (`crates/cache-stability/tests/golden_vectors.rs:160-166`), `cross_episode_lineage_reproduces_byte_identical` (`crates/cache-stability/tests/golden_vectors.rs:263`); all pass on Rust 1.89 and stable in the destination workspace.
- **Failure scenario:** a regenerated fixture passes its own test; only the external byte comparison detects a changed contract.
- **Timing window:** none.
- **Instrumentation:** the receipt checker recomputes the destination hash and compares it to `git cat-file` of the source blob on every run.
- **Audit verdict (U2): pass. Independent oracle with one noted limit: `run_vector` (`crates/cache-stability/tests/golden_vectors.rs:168`) passes `expect_action` as the `proposed` action, so the action-equality assertion is near-tautological. The assertions on `cached_prefix_bytes()`, `boundary_id`, `reconcile_pending`, and the pending-change count are independent of the input and carry the byte-stability claim; the in-crate mechanism tests cover the classifier-independent arms.
- **Open-question log:** none.
