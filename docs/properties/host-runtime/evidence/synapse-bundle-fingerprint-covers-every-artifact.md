# `synapse-bundle-fingerprint-covers-every-artifact`

- **Discovery:** U3, when the pre-image's first line was renamed.
- **Primary evidence:** `canonical_fingerprint` in `crates/host-runtime/src/synapse/bundle.rs` and its mirror in `tests/fixtures/generate-synapse-tiny.py`. The fixture manifest's fingerprint `2bba4ff1399076304377c063fbccac0709daf89d183ab90e712a36c06ae42b5f` was produced at U3 by the generator's Python function over the committed manifest; the same function reproduced the predecessor value from the predecessor line, so only the renamed line moved the digest and the artifacts are unchanged.
- **Existing evidence:** `the_committed_fixture_carries_its_canonical_fingerprint`, `a_bundle_manifest_outside_the_committed_digest_does_not_load`, `one_bit_changes_to_each_artifact_disable_the_lane` (`crates/host-runtime/tests/synapse_bundle.rs`).
- **Failure scenario:** an artifact omitted from the pre-image.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The one-bit tests show each artifact's own digest is checked at load, before the fingerprint comparison; that the pre-image lists every artifact is established by reading `canonical_fingerprint` against its Python mirror, not by a test.
- **Open-question log:** none.
