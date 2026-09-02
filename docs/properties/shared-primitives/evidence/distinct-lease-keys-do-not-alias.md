# `distinct-lease-keys-do-not-alias`

- **Discovery:** architecture, security, and protocol-format passes.
- **Primary evidence:** public `LeaseKey` fields and constructor feed `LeaseKey::identity`; `FileLeaseStore::lease_path` hashes that identity with public `fnv1a_hex`. PostgreSQL uses the same `fnv1a(key.identity())` value for its advisory lock and epoch row (`commons@89abb40 crates/`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:65-78`,332-359`).
- **Confirmed witness:** `("a\u{1f}b","c","d")` and `("a","b","c\u{1f}d")` join to the same identity. Fields are public and unvalidated.
- **Additional mechanism:** FNV-1a-64 has no collision detection and the file stores no full identity. Practical targeted-collision cost was not established.
- **Existing evidence:** file-backend separation tests use only separator-free values. `identity_hash_derivation_is_stable` and PostgreSQL's `advisory_key_derivation_is_stable` provide golden assertions; neither covers separator-bearing tuples or hash collisions.
- **Failure scenario:** distinct stores share the file lock path or PostgreSQL advisory lock and epoch row, causing false `Held`, a shared epoch counter, or false fencing.
- **Instrumentation:** identity and hash outputs are already publicly observable and covered by golden tests. Missing evidence is an adversarial separator-bearing tuple test plus a stored full-key binding or collision check at the file path and PostgreSQL epoch row.
- **Open-question log:** `StorageDescriptor` fields are deserialized strings (`crates/storage-types/src/lib.rs:75-89`), but deployed value constraints are outside this repository. Descriptor-derived backend labels are closed to the `Sqlite` and `Postgres` enum variants (`crates/storage-types/src/lib.rs:49-73`), although callers can still construct a public `LeaseKey` with an arbitrary backend string.
