# `storage-descriptor-golden-vectors-are-byte-stable`

- **Discovery:** golden-vector pass at U2.
- **Primary evidence:** `crates/storage-types/tests/golden/storage_vectors.json` is authored in this tree (`source: null`) by `cargo run -p storage-types --example golden-vectors` for seven sample module ids (`module-a`, `module-b`, `module-c`, `module-d`, `a-b`, `a_b`, and one overlong id). Its receipt entry in `migration/waves/U2/receipt.json` records `destination_sha256`, which pins the bytes; the registry lists the fixture as `byte-stable` and `examples/golden-vectors.rs` as its generator, so the receipt checker accepts only `verbatim` or `authored` as its transformation. Changing the file is a reviewed contract change under R18.
- **Existing evidence:** `helpers_reproduce_the_golden_vectors` (`crates/storage-types/tests/golden_vectors.rs:11-42`) asserts `postgres_database_name`, `sqlite_store_path`, and descriptor reserialization for all seven vectors; `golden_vectors_break_slug_collisions` (`crates/storage-types/tests/golden_vectors.rs:44-61`) asserts `a-b` and `a_b` differ.
- **Failure scenario:** a changed `eidnara_` prefix, `eidnara/` path component, or serde tag passes a regenerated fixture and fails only the receipt hash comparison.
- **Timing window:** none.
- **Instrumentation:** receipt verification recomputes the destination hash and compares it to the receipt.
- **Audit verdict (U2): pass. The fixture is read with `include_str!` and parsed independently of the helpers; each derivation is compared to a literal from the file, not to another call of the same helper.
- **Open-question log:** the PostgreSQL name derivation has no consumer in this workspace; retiring it is a contract change that must replace this fixture (architecture candidate C1 in `migration/waves/U2/architecture-impact.json`).
