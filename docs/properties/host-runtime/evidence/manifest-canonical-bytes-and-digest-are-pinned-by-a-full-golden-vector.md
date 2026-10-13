# manifest-canonical-bytes-and-digest-are-pinned-by-a-full-golden-vector

## Discovery trigger

Gap G4 from `portfolio-evaluation.md`: "Manifest bytes depend on struct
declaration order, and only predecessor-with-missing-field compatibility is
golden. A full byte-and-digest vector is missing." The task was to verify what the
existing golden actually pins and to catalog the vector that would close it.

## Evidence trail

The encoding is declaration order, with nothing between the struct and the bytes:

```
// generation.rs:262-264
pub fn canonical_bytes(&self) -> Vec<u8> {
    serde_json::to_vec(self).expect("manifest serialization cannot fail")
}

// generation.rs:266-268
pub fn digest(&self) -> String {
    hex(&sha2::Sha256::digest(self.canonical_bytes()))
}
```

`serde_json::to_vec` on a derived `Serialize` emits fields in declaration order,
so the byte order is exactly:

- `GenerationManifest`, `generation.rs:235-247`: `schema` (`:236`), `target`
  (`:237`), `release_contract_sha256` (`:238`), `inputs_lock_sha256` (`:239`),
  `source_payload_manifest_sha256` (`:245`, attribute at `:244`), `files` (`:246`).
- `ManifestFile`, `generation.rs:219-224`: `path` (`:220`), `mode` (`:221`),
  `size` (`:222`), `sha256` (`:223`).

The struct's own doc comments at `:226` and `:231-232` state the contract:
"`GenerationManifest` serializes fields in declaration order", "`files` is sorted by
path, making the encoding deterministic", and "`GenerationManifest`'s SHA-256
names the generation."

The existing golden is `generation.rs:2111-2121`,
`a_generation_staged_before_the_source_digest_field_still_decodes`. Its fixture is
the literal at `:2112`:

```
{"schema":1,"target":"linux-x64-gnu","release_contract_sha256":"aa","inputs_lock_sha256":"bb","files":[]}
```

and its byte assertion is `:2091`:

```
assert_eq!(decoded.canonical_bytes(), predecessor);
```

That is a genuine byte-exact vector, and it is the only one in the crate. A search
for a hardcoded manifest SHA-256 in a test found none;
`UNSTAGED_PAYLOAD_MANIFEST_DIGEST` (`config.rs:84`) is a config default, not a
manifest golden.

Reachability label evidence: `default-production`. The store's stage and validate
paths are on the default start path, and `config.rs:292` supplies a default
`payload_manifest_digest`, so no opt-in gates the encoding. The catalog preamble
already classes the payload generation store as default-production.

## Failure scenario

Three concrete edits leave the existing test green and change real generations'
bytes.

1. Move `source_payload_manifest_sha256` up beside the other two hashes, which is
   where a reader would expect it. The fixture omits the field, and `:237` marks
   it `skip_serializing_if = "Option::is_none"`, so the fixture's bytes are
   unchanged. Every generation staged with a source now encodes as
   `{"schema":..,"target":..,"release_contract_sha256":..,"source_payload_manifest_sha256":..,"inputs_lock_sha256":..,"files":..}`
   and hashes to a different name.
2. Alphabetize `ManifestFile` to `mode, path, sha256, size`. The fixture has
   `"files":[]`, so nothing in the test observes the change. Every generation with
   at least one file changes bytes, which in practice is all of them.
3. Change the digest input or the hex encoding at `:259-261`. No test asserts a
   digest value at all, so this is invisible to the suite.

Edit 3's blast radius is the widest and its detection is the weakest.

## Timing windows and dependencies

None. This is a static encoding contract with no concurrency, no filesystem, and
no fault. The record is included because the cost of the check is one unit test
and the cost of the defect is a forward-compatibility break, which the sibling
record `a-declaration-order-change-cannot-orphan-a-retained-generation` covers.

One dependency worth naming: the check depends on `serde_json` preserving
declaration order for derived structs, which it does for a struct without
`#[serde(rename_all)]` or field reordering attributes. Neither struct has one. A
future `serde_json` change to key ordering would break the golden, which is
correct behaviour for the golden: the contract is the bytes, not the library.

## What a test must construct

A single `#[test]` in `generation.rs`'s test module, no store and no filesystem:

1. Build a `GenerationManifest` with all four leading scalars set to distinct
   recognizable values, `source_payload_manifest_sha256: Some(..)`, and `files`
   holding at least two `ManifestFile` entries with distinct paths in sorted
   order, so field order inside `ManifestFile` and the sortedness of `files` are
   both observable.
2. Assert `manifest.canonical_bytes() == GOLDEN_BYTES` where `GOLDEN_BYTES` is a
   byte-string literal.
3. Assert `manifest.digest() == GOLDEN_DIGEST` where `GOLDEN_DIGEST` is a
   64-character lowercase hex literal.
4. Assert the reverse direction: decode `GOLDEN_BYTES` through
   `decode_with_schema::<GenerationManifest>` and re-encode, asserting the bytes
   round-trip. This catches an encoding change that a decoder change happens to
   mask.

Keep the existing predecessor test as well. The two cover different evolutions:
the existing one covers a field absent, the new one covers every field present.

The comment on the new test should say what a failure means, because a maintainer
who reorders a field will see it fail and needs to know that the correct response
is to revert the reorder, not to update the golden.

## Investigation log

### Q: Does the existing golden catch anything, or is it purely blind?

- Sources examined: `generation.rs:2111-2121` in full, `:219-224`, `:235-243`,
  `:240` (the skip attribute), `:234` and `:218` (`deny_unknown_fields` on both
  structs), `decode_with_schema` at `:273-286`.
- Findings: it catches two real classes. First, reordering the four leading
  scalars or moving `files` among them changes the fixture's own bytes, so the
  assertion at `:2091` fails. Second, adding a *required* field breaks the decode:
  `deny_unknown_fields` is not the mechanism there, a missing field is, and
  `decode_with_schema` returns `Malformed`, which the test converts to an explicit
  panic at `:2085-2087`. So the crudest mistake is defended.
- Missing evidence: none needed.
- Conclusion: resolved with answer. The existing golden is not blind; it is
  partial, in exactly the two places a careful author is most likely to touch. The
  record's `Exercised:` field says `partial` for that reason rather than `not yet`.
