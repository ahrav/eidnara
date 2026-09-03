# `harness-closure-manifest-digest-is-canonical`

- **Discovery:** U3, when the fixture's schema id was renamed.
- **Primary evidence:** `manifest_digest` in `crates/host-runtime/src/harness_closure.rs` is SHA-256 over `serde_json::to_vec_pretty(sort_json(manifest))`. The committed fixture `tests/fixtures/harness-closures/pi-valid.json` digests to `5386c2004cc31abbdd98e766be193f78e1a74937254681e6db47bd700961f911`; a Python `sha256(json.dumps(manifest, sort_keys=True, indent=2))` produced that value and reproduced the predecessor value `4043614c...5e51` from the predecessor schema string, so the canonical form is unchanged.
- **Existing evidence:** `rust_and_typescript_share_the_canonical_manifest_digest` and the strict-decode tests in `crates/host-runtime/tests/harness_closure.rs`.
- **Failure scenario:** order-dependent or non-canonical serialization.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The literal was produced outside the crate. The TypeScript twin that shares this digest lands with the packages in U7 and must read this fixture.
- **Open-question log:** the TypeScript side of the shared digest is not in this tree yet.
