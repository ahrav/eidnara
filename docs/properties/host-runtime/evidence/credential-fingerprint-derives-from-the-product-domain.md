# `credential-fingerprint-derives-from-the-product-domain`

- **Discovery:** U3, when the domain separator was renamed.
- **Primary evidence:** `EnvSnapshot::credential_fingerprint` in `crates/host-runtime/src/broca/subprocess.rs`. The vector `ecac831b94bb1d9e972ee993f7798c9ff7c6133b545e489ac1a3f60448127e80` was produced at U3 by a Python implementation of the documented derivation (derive key by HMAC over the domain, then HMAC over the length-prefixed canonical row); the same script reproduced the predecessor value from the predecessor domain.
- **Existing evidence:** `credential_fingerprint_matches_the_committed_vector` (`crates/host-runtime/src/broca/subprocess.rs`, added at U3 so the checker has an anchored test) and `provider_rows_exclude_ambient_credentials_and_enforce_caps` in `crates/host-runtime/tests/broca_subprocess.rs`, a `harness = false` binary whose runner names its checks as plain functions.
- **Failure scenario:** a fingerprint that matches across products or leaks the credential.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The literal was produced outside the crate; the unit test also shows a different connection key changes the digest.
- **Open-question log:** none.
