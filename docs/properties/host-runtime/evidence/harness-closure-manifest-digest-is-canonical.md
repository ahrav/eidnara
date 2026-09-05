# harness-closure-manifest-digest-is-canonical

## Discovery trigger

The closure manifest's `schema` field was renamed at U3 to
`eidnara.host-harness-closure/v1`, which moved the committed fixture's
digest. The digest is the identity under which a materialized closure is
stored, looked up, and verified, so two properties must hold at once: equal
manifests must digest equal regardless of how their JSON was written, and the
canonical form must be one that the TypeScript packaging side, which lands
with U7 and reads the same fixture, can reproduce. The audit traced the
canonical serialization, the digest's use as a store key, and the fixture
test.

## Evidence trail

All references are at `572315a`.

Canonical form. `manifest_digest` (`harness_closure.rs:245-252`) first runs
`validate_manifest` (`:246`), which rejects any `schema` other than
`CLOSURE_SCHEMA` (`:23`, `:288-290`), then serializes with
`canonical_manifest` (`:254-259`): `serde_json::to_value` of the struct,
`sort_json` over the value, `serde_json::to_vec_pretty` of the result. The
byte length is capped at `MAX_MANIFEST_BYTES` (16 MiB, `:25`, `:248-250`)
and the SHA-256 is hex-encoded (`:251`, `hex` at `:1138`). `sort_json`
(`:261-285`) recurses into arrays, sorts object keys, and passes scalars
through. Arrays keep their order, so `extensions` and `nodes` ordering is
part of the identity. The workspace does not enable serde_json's
`preserve_order` feature (workspace `Cargo.toml:28`, `Cargo.lock`), so even
before `sort_json` the map is key-sorted; `sort_json` makes that explicit.

Struct. `ClosureManifest` (`:34-49`) is `deny_unknown_fields` and every
field is serialized; `executable`, `interpreter`, and `entrypoint` are
`Option<String>` and serialize as `null` when absent, which is how the
fixture writes `executable`. `ClosureNode` (`:51-62`) and
`ClosureDependency` (`:64-69`) are also `deny_unknown_fields`.

Fixture. `tests/fixtures/harness-closures/pi-valid.json` carries the schema
string on its second line and five nodes. The audit reproduced the committed
digest with Python: `sha256(json.dumps(manifest, sort_keys=True,
indent=2))` over the parsed fixture gives
`5386c2004cc31abbdd98e766be193f78e1a74937254681e6db47bd700961f911`. Python's
`indent=2` output and serde_json's `to_vec_pretty` agree on separators,
indentation, and the absence of a trailing newline for this input.

Store. `HarnessClosureStore::materialize` (`:539-580`) computes the digest at
`:545` and uses it as the directory name. `stage_candidate` writes the
canonical bytes as `manifest.json` (`:666-667`). `validate` (`:595-633`)
reads the retained manifest, requires `sha256(bytes) == digest`
(`:601-603`), decodes it, and requires `canonical_manifest(&manifest) ==
bytes` (`:607-610`), so a retained manifest that is not in canonical form is
rejected even if its digest matches. The store is opened only from tests in
this tree (`tests/broca_subprocess.rs:845`, `tests/harness_closure.rs`); the
backends consume a `ValidatedHarnessClosure` (`broca/opencode.rs:28`,
`broca/pi.rs:44`).

Existing checks, verified, in `tests/harness_closure.rs`, a default-harness
binary CI runs via `cargo test --workspace --all-targets`
(`.github/workflows/ci.yml:118`):

- `canonical_manifest_digest_is_pinned` (`:429-439`) decodes the fixture
  through `ClosureManifest` and asserts `manifest_digest` equals the literal
  (`:435-438`). The name was changed at U3 from one that claimed
  cross-language agreement; no test in this tree performs that comparison.
- `ordered_extensions_are_part_of_manifest_identity` (`:408-415`) reverses
  `extensions` and asserts the digest changes.
- `strict_manifest_decode_rejects_unknown_fields` (`:418-426`) adds an
  `ambient_path` key and asserts decoding fails.
- `source_and_retained_hash_mismatches_fail_closed` (`:325`) and
  `retained_closure_rejects_extra_missing_and_wrong_mode_nodes` (`:499`)
  cover the store's use of the digest; the record does not list them.

## Failure scenario

1. A packaging tool writes the manifest with keys in declaration order and
   the host stores it under a digest of those bytes.
2. A second tool, or the same tool after a struct field is reordered, emits
   the same fields in a different order. A digest over the raw bytes now
   names a different directory for an identical closure, so deduplication
   fails and `validate` cannot find the retained copy.
3. As written, both serializations decode to the same `ClosureManifest`,
   `sort_json` orders the keys identically, and the digest is the same. The
   fixture test would catch a change to the canonical form itself, because
   the literal was produced outside the crate.

## Timing windows and dependencies

None on the digest. The canonical form depends on serde_json's pretty
printer: a change to its indentation or separator bytes would move every
digest and would be caught by the fixture test. The cross-language contract
depends on the TypeScript side using the same serializer conventions; that
side is not in this tree.

## What a test must construct

The record's check has three parts, each with a test in
`tests/harness_closure.rs`:

- `canonical_manifest_digest_is_pinned` covers
  `manifest_digest(fixture) == committed`.
- `manifest_digest_is_stable_under_key_reordering` rewrites the fixture as
  JSON text with every object's keys in reverse order, asserts the text
  differs from serde's key-sorted output, decodes it, and asserts the digest
  equals the fixture's. This pins the key-order clause against a future
  `preserve_order` feature or a raw-bytes digest.
- `manifest_digest_changes_when_any_field_changes` changes one field at a
  time while keeping the manifest valid (`harness`, `package`, `version`,
  `argument_variant`, `source_roots`, the extension list, the node count,
  and a node's `path`, `source_root`, `source_path`, `kind`, `sha256`,
  `size_bytes`, dependency `kind`, dependency `path`, and dependency count)
  and asserts the digest moves and no two mutations collide. A foreign
  `schema` and a `mode` that disagrees with its `kind` are shown to be
  refused before hashing, since the validator fixes both.
- `manifest_digest_matches_an_external_canonicalization_of_the_fixture_text`
  reproduces the digest from the fixture's JSON text with a test-local key
  sort, independent of the crate's `Serialize` impl, and counts each node
  path in the canonical text. The validator requires every node to be
  referenced by a launch root, an extension, or a dependency edge, so no
  in-crate mutation can move a node path alone; the external comparison is
  what catches a canonical form that dropped one.
- `launch_roots_participate_in_the_digest_on_their_own` builds a manifest
  with an alternate interpreter node and an alternate entrypoint node, both
  reachable through the extension root, and changes `interpreter` and
  `entrypoint` each alone; it then converts the same manifest to the
  executable launch form and moves `executable` alone between the two
  executable nodes, so none of the three launch fields can hide behind the
  node path it names. `ordered_extensions_are_part_of_manifest_identity`
  covers extension order separately.

## Investigation log

### Q: Does the digest cover the serialized struct or the fixture bytes?

- Sources examined: `harness_closure.rs:245-259`, `:595-610`;
  `tests/harness_closure.rs:429-439`.
- Findings: `manifest_digest` digests `canonical_manifest`, which is the
  struct re-serialized, not the input bytes. The fixture test decodes first,
  so the fixture's own formatting is irrelevant to the pinned value. The
  store's `validate` requires retained bytes to be byte-equal to the
  canonical form, so the stored copy is always the canonical bytes.
- Missing evidence: none.
- Conclusion: resolved. The digest is over the canonical re-serialization.

### Q: Does an independent implementation reproduce the digest?

- Sources examined: the fixture file; `harness_closure.rs:254-285`; the
  Python reproduction described above.
- Findings: `json.dumps(sort_keys=True, indent=2)` reproduces the literal.
  The U3 summary states the same script reproduced the predecessor value
  from the predecessor schema string; the predecessor string is not in this
  tree, so that half is not re-verified here.
- Missing evidence: the TypeScript twin, which lands with the packages in U7
  and must read this fixture.
- Conclusion: resolved for Python agreement; unresolved for the TypeScript
  side, needs the U7 implementation.
