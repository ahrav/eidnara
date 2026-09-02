# `cache-stability-golden-vectors-are-byte-stable`

- **Discovery:** golden-vector pass at U2; the fixture is the cross-harness contract.
- **Primary evidence:** `crates/cache-stability/tests/golden/cache-stability-golden-vectors.json` is authored in this tree (`source: null`). Its receipt entry in `migration/waves/U2/receipt.json` records `destination_sha256`, which pins the bytes; the registry lists the file as a `byte-stable` fixture, so the receipt checker accepts only `verbatim` or `authored` as its transformation. Changing the file is a reviewed contract change under R18.
- **Existing evidence:** `golden_fixture_is_schema_v3_with_eleven_vectors` (`crates/cache-stability/tests/golden_vectors.rs:97`), `core_state_schema_v3_empty_wire_format_is_stable` (`crates/cache-stability/tests/golden_vectors.rs:108`), `all_golden_vectors_pass` (`crates/cache-stability/tests/golden_vectors.rs:160-166`), `cross_episode_lineage_reproduces_byte_identical` (`crates/cache-stability/tests/golden_vectors.rs:275`); all pass on Rust 1.98 and stable in this workspace and reproduce every vector from the in-tree core.
- **Failure scenario:** a regenerated fixture passes its own test; only the receipt hash comparison detects a changed contract.
- **Timing window:** none.
- **Instrumentation:** the receipt checker recomputes the destination hash and compares it to the receipt's `destination_sha256` on every run.
- **Audit verdict (U2): pass. Independent oracle with one noted limit: `run_vector` (`crates/cache-stability/tests/golden_vectors.rs:168`) passes `expect_action` as the `proposed` action, so the action-equality assertion is near-tautological. The assertions on `cached_prefix_bytes()`, `boundary_id`, `reconcile_pending`, and the pending-change count are independent of the input and carry the byte-stability claim; the in-crate mechanism tests cover the classifier-independent arms.
- **Open-question log:** none.
