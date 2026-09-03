# `lease-path-format-is-version-stable`

- **Discovery:** version-compatibility and protocol passes.
- **Primary evidence:** public `LeaseKey::identity`, `fnv1a`, and `fnv1a_hex` form the compatibility contract; private `FileLeaseStore::lease_path` appends the `.lease` suffix.
- **Cross-crate evidence:** PostgreSQL imports public `fnv1a` and hashes public `LeaseKey::identity` in `advisory_key` (`primitives@89abb40`).
- **Existing evidence:** `identity_hash_derivation_is_stable` provides golden identity/hash coverage, and the source's PostgreSQL backend (`primitives@89abb40`) pins the resulting advisory key in `advisory_key_derivation_is_stable`.
- **Residual gaps:** no automated SemVer gate, mixed-version overlap test, full-filename golden, or adversarial vectors.
- **Failure scenario:** rolling restart or rollback overlaps binaries using different separators, field order, normalization, hash, or suffix.
- **Timing window:** from first new-version acquisition until every old process is gone.
- **Instrumentation:** artifact-version and derived-path observations remain missing.
- **Residual risk:** one edit to the shared identity or FNV-1a derivation can remap both file and PostgreSQL lock domains.
- **Open-question log:** mixed-version overlap policy is not documented. The versioning rule is stated in the source README (`primitives@89abb40`); `crates/lease/Cargo.toml:3` records version `0.3.0`, with no path-derivation change since `0.2.0`.

## U2 audit

- **Classification:** `core`. The identity encoding, the FNV-1a-64 digest, and the `.lease` suffix are what existing lease files depend on.
- **New evidence:** `lease_path_vectors_are_version_stable` (`crates/lease/src/lib.rs:1933-1989`) pins six sample keys against digests computed by an independent FNV-1a-64 implementation (Python), not by the crate: `("test-module", "sqlite", "main")` → `51a7eaa424b9fd8f` (the same vector `identity_hash_derivation_is_stable` pins), `("module-a", "sqlite", "core")` → `0160e3525823870e`, `("module-b", "sqlite", "cache")` → `b9ebf913322ef03a`, empty fields, non-ASCII fields, and a 300-byte field. No vector carries `U+001F` inside a field, because `LeaseKey::identity` rejects the separator in every field; `separator_in_a_key_field_fails_closed_instead_of_aliasing` (`crates/lease/src/lib.rs:2105-2128`) pins that rejection. The test also acquires the `module-a` key and asserts the directory then contains exactly `0160e3525823870e.lease`.
- **Discrimination:** mutants that change the suffix to `.lock` or the separator to `|` fail this test and `identity_hash_derivation_is_stable`.
- **Verdict:** pass for the path format under both toolchains on `x86_64-unknown-linux-gnu`. Cross-version overlap policy and the advisory-key vector of the source's PostgreSQL backend remain open, as the record states.
