# `descriptor-wire-shape-round-trips`

- **Discovery:** storage-types wire pass.
- **Primary evidence:** `Isolation` and `StorageBackend` carry `#[serde(rename_all = "snake_case", tag = ...)]` (`crates/storage-types/src/lib.rs:40-62`); `StorageDescriptor` (`crates/storage-types/src/lib.rs:77-89`) derives `Serialize` and `Deserialize` with default field names.
- **Existing evidence:** `sqlite_descriptor_golden_json` (`crates/storage-types/src/lib.rs:167-183`) compares against the exact JSON string and round-trips; `postgres_descriptor_golden_json` (`crates/storage-types/src/lib.rs:186-200`) round-trips the other variant; `helpers_reproduce_the_golden_vectors` reserializes eight fixture descriptors.
- **Failure scenario:** a renamed field or tag breaks descriptor exchange between a host and a module built from different revisions.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The SQLite case compares to a literal string, so any attribute change fails it; the round trip checks `PartialEq`, which covers every field.
- **Open-question log:** none.
