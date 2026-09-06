# `storage-descriptor-golden-vectors-are-byte-stable`

- **Discovery:** golden-vector pass at U2.
- **Primary evidence:** `crates/storage-types/tests/golden/storage_vectors.json` is authored in this tree by `cargo run -p storage-types --example golden-vectors` for seven sample module ids (`module-a`, `module-b`, `module-c`, `module-d`, `a-b`, `a_b`, and one overlong id) and tracked byte for byte. Changing the file is a reviewed contract change under R18; the change shows as a diff to the fixture.
- **Existing evidence:** `helpers_reproduce_the_golden_vectors` (`crates/storage-types/tests/golden_vectors.rs:11-42`) asserts `postgres_database_name`, `sqlite_store_path`, and descriptor reserialization for all seven vectors; `golden_vectors_break_slug_collisions` (`crates/storage-types/tests/golden_vectors.rs:44-61`) asserts `a-b` and `a_b` differ.
- **Failure scenario:** a changed `eidnara_` prefix, `eidnara/` path component, or serde tag passes a regenerated fixture and shows only as a diff to the tracked fixture.
- **Timing window:** none.
- **Instrumentation:** no automated instrumentation. The test proves that the in-tree helpers reproduce the fixture; nothing compares the fixture bytes to an in-code expectation. A regenerated fixture is caught only when a reviewer reads its diff.
- **Audit verdict (U2): pass. The fixture is read with `include_str!` and parsed independently of the helpers; each derivation is compared to a literal from the file, not to another call of the same helper.
- **Open-question log:** the PostgreSQL name derivation has no consumer in this workspace; retiring it is a contract change that must replace this fixture (architecture candidate C1 in the U2 wave note).
