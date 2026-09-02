# `storage-descriptor-golden-vectors-are-byte-stable`

- **Discovery:** golden-vector pass at U2.
- **Primary evidence:** destination `crates/storage-types/tests/golden/storage_vectors.json` sha256 `d3c2d773d8b8d35e0216463ba2408e35d5d83e9dc2c7d1d60ef112103648ecd8`; source blob `6aa91fa1ca848ccb1d3641a428887072501ed964` at `commons@89abb40` has the same content sha256. The receipt entry is `verbatim`, the registry lists the fixture as `byte-stable` and `examples/golden-vectors.rs` as its generator.
- **Existing evidence:** `helpers_reproduce_the_golden_vectors` (`crates/storage-types/tests/golden_vectors.rs:12-43`) asserts `postgres_database_name`, `sqlite_store_path`, and descriptor reserialization for all eight vectors; `golden_vectors_break_slug_collisions` (`crates/storage-types/tests/golden_vectors.rs:45`) asserts `a-b` and `a_b` differ.
- **Failure scenario:** a changed `cortexkit_` prefix, `cortexkit/` path component, or serde tag passes a regenerated fixture and fails only the byte comparison.
- **Timing window:** none.
- **Instrumentation:** receipt verification recomputes both hashes.
- **Audit verdict (U2): pass. The fixture is read with `include_str!` and parsed independently of the helpers; each derivation is compared to a literal from the file, not to another call of the same helper.
- **Open-question log:** the PostgreSQL name derivation has no consumer in this workspace; retiring it is a contract change that must replace this fixture (architecture candidate C1 in `migration/waves/U2/architecture-impact.json`).
