# synapse-bundle-fingerprint-covers-every-artifact

## Discovery trigger

The fingerprint pre-image's first line was renamed at U3 to
`eidnara-synapse-fingerprint-v1`, so the committed tiny fixture's manifest
fingerprint was regenerated once. The fingerprint is the lane identity under
which embeddings are served and stored, so it must change whenever any input
that can change a served vector changes. The threat is an artifact that is
verified by its own SHA-256 at load but omitted from the fingerprint
pre-image: a swapped artifact would then serve different vectors under an
unchanged identity. The audit traced the pre-image line by line against its
Python mirror, traced load-time verification of each artifact, and checked
what the fixture tests actually assert.

## Evidence trail

All references are at `572315a`.

Pre-image. `canonical_fingerprint` (`synapse/bundle.rs:566-614`) starts the
pre-image with `eidnara-synapse-fingerprint-v1` (`:577`) and appends
newline-separated `key=value` lines (`:578-583`): `model_file` sha256
(`:584`), one `external_initializer` line per initializer as
`{name_len}:{name}:{sha256}` (`:585-594`), the four tokenizer artifact
hashes `tokenizer`, `config`, `special_tokens_map`, `tokenizer_config`
(`:595-604`), then `pooling`, `quantization`, `output`, `max_tokens`,
`dims`, `table_epoch` (`:605-611`), and `corpus` sha256 (`:612`), and hashes
with SHA-256 (`:613`). The output selector is rendered as `name:`, `index:`,
`only_one`, or `unselected` (`:567-576`). The doc comment at `:563` states
that `model`, `provenance`, and `recommended_batch` are excluded because
they cannot change a served vector. Length-prefixing initializer names
(`:565`) stops a filename containing `:` from forging a field.

Python mirror. `tests/fixtures/generate-synapse-tiny.py:115-157` defines
`canonical_fingerprint` with the same lines in the same order and the
docstring at `:118-125` names the Rust function as the source of truth. The
generator writes the result into the manifest at `:235-244`.

Fixture. `tests/fixtures/synapse-tiny/manifest.json` carries `fingerprint`
`2bba4ff1399076304377c063fbccac0709daf89d183ab90e712a36c06ae42b5f` and lists
seven artifacts: `model.onnx`, `embedding.bin`, the four tokenizer files,
and `corpus.json`. The audit reproduced the fingerprint in Python from the
manifest's fields following the Rust layout, and confirmed each of the seven
files' SHA-256 matches its manifest entry.

Load-time verification. `load_bundle` (`:175-281`) optionally checks the
manifest bytes against a generation-committed digest (`:188-194`), rejects
duplicate artifact names and unlisted directory entries (`:203-214`), reads
every artifact through `read_verified_open` (`:683-697`), which returns
`artifact hash mismatch: {name}` when the bytes' SHA-256 differs from the
manifest entry (`:693-695`), and only after all artifact and field checks
compares `manifest.fingerprint` to `canonical_fingerprint` (`:262-268`). The
fingerprint string's shape is validated at `:297`. The production caller is
`SynapseComponent::activate` at `synapse/mod.rs:1025-1029`, with the
generation's committed digest passed as `bundle_manifest_sha256`.

Existing checks, verified, in `tests/synapse_bundle.rs`, a default-harness
binary CI runs via `cargo test --workspace --all-targets`
(`.github/workflows/ci.yml:118`):

- `the_committed_fixture_carries_its_canonical_fingerprint` (`:375-389`)
  loads the fixture with `load_bundle` and asserts
  `bundle.manifest.fingerprint == canonical_fingerprint(&bundle.manifest)`
  (`:384-388`). Because `load_bundle` already enforces that equality at
  `:262-268`, a successful load implies the assertion; the test's value is
  that it fails with a fixture-specific message. It does not compare the hex
  literal.
- `a_bundle_manifest_outside_the_committed_digest_does_not_load`
  (`:395-417`) loads with the fixture manifest's own digest (`:401`) and
  then with a different digest, asserting the rejection text (`:404-416`).
- `one_bit_changes_to_each_artifact_disable_the_lane` (`:282-305`) flips the
  last byte of each of the seven artifacts in a copy of the fixture and
  asserts the lane is disabled with `hash mismatch` (`:293-303`). The helper
  `expect_disabled_with` (`:105-116`) uses a nonexistent ORT library path
  (`:79-84`), so the check does not depend on a native runtime.
- `a_stale_fingerprint_disables_the_lane` (`:358-372`) edits `fingerprint`
  to a well-formed wrong value and, separately, edits `table_epoch` without
  updating `fingerprint`, asserting the canonical-fingerprint rejection in
  both cases. The record does not list this test.
- `fingerprint_binds_initializer_names_to_their_hashes`
  (`synapse/bundle.rs:898-923`) swaps two initializer names while keeping
  their hashes and asserts the fingerprint changes.

## Failure scenario

1. A new tokenizer side file is added to the manifest and verified by SHA-256
   at load, but no line is added to `canonical_fingerprint`.
2. A bundle author swaps that file for one that changes tokenization. Every
   artifact hash matches its own entry, the fingerprint is unchanged, and the
   lane serves different vectors under the same identity.
3. As written, the pre-image covers all seven artifact hashes and every
   embedding-space scalar. A one-bit change to any artifact is caught first
   by its own hash at load; a manifest edit that changes an artifact hash
   without updating `fingerprint` is caught at `:262-268`.

The exposure the record names is real for any future artifact: nothing but
review of `canonical_fingerprint` against the manifest struct enforces that
a new `ArtifactRef` field is folded in.

## Timing windows and dependencies

None. Both the fingerprint and the artifact checks are synchronous inside
`load_bundle`, which runs on a blocking task during activation
(`synapse/mod.rs:1024`). The generator's mirror must move in the same edit
as the Rust function (`generate-synapse-tiny.py:122-125`); the fixture test
detects a Rust-side change because the committed manifest would then fail to
load, and it detects a generator-side change only when the fixture is
regenerated.

## What a test must construct

Two checks establish the pre-image's coverage in code rather than by reading
the function against its Python mirror:

- `every_artifact_hash_and_embedding_scalar_participates_in_the_fingerprint`
  (the `#[cfg(test)]` module of `bundle.rs`) changes each artifact `sha256`
  of the test manifest alone (`model_file`, both `external_initializers`, the
  four tokenizer artifacts, `corpus`), each external-initializer `name`
  alone (one rename to `wëights.bin`, whose byte length differs from its
  character count, so a character-counted length prefix diverges), and each
  embedding-space scalar alone
  (`pooling`, `quantization`, each `output` selector form and the numeric
  `output.index` value under an unchanged tag, `max_tokens`, `dims`,
  `table_epoch`), validates each mutated manifest so every case is a loadable
  bundle, and asserts `canonical_fingerprint` changes and
  that no two mutations produce the same fingerprint. An input added to
  `BundleManifest` but omitted from the pre-image fails the test once its
  mutation is listed. `fingerprint_binds_initializer_names_to_their_hashes`
  covers the remaining structural case: a swap of two initializer names with
  every hash held fixed (the pre-image binds `name.len():name:sha256`,
  `bundle.rs:585-594`).
- `the_committed_fixture_carries_its_canonical_fingerprint` pins the literal
  `2bba4ff1399076304377c063fbccac0709daf89d183ab90e712a36c06ae42b5f`, so a
  regenerated fixture whose pre-image changed fails against the literal, not
  only against the recomputation the manifest also carries.

The per-artifact file test still works at the file level and is intercepted
by the artifact hash check before the fingerprint comparison; it proves
artifact integrity, not fingerprint coverage.

## Investigation log

### Q: Does any Rust test compare the fingerprint to an external literal?

- Sources examined: `tests/synapse_bundle.rs:375-389`, `:395-417`;
  `synapse/bundle.rs:262-268`; the fixture manifest.
- Findings: `the_committed_fixture_carries_its_canonical_fingerprint` pins the
  hex literal after the recomputation check. The external agreement is that
  the fixture manifest was written by the Python generator and `load_bundle`
  recomputes the same value in Rust; the literal adds a third point that a
  fixture regenerated by an edited generator alongside an edited Rust function
  cannot move silently. The audit's own Python reproduction confirms the value.
- Missing evidence: none.
- Conclusion: resolved. Agreement is proven through the fixture load and the
  pinned literal.

### Q: Does the one-bit artifact test prove the pre-image covers each artifact?

- Sources examined: `tests/synapse_bundle.rs:282-305`;
  `synapse/bundle.rs:230-257`, `:262-268`, `:683-697`.
- Findings: each artifact is read through `read_verified_open` before the
  fingerprint comparison, so the disabling reason is `hash mismatch`, which
  the test asserts. The fingerprint line for that artifact is never reached.
  The test proves load-time artifact integrity, not pre-image coverage.
- Missing evidence: none.
  `every_artifact_hash_and_embedding_scalar_participates_in_the_fingerprint`
  mutates each manifest `sha256` field, each external-initializer name, and
  each embedding-space scalar alone and asserts the fingerprint moves.
- Conclusion: resolved. Coverage of every artifact by the pre-image is proven
  by the struct-level mutation test; the reading of `:584-612` against the
  mirror is corroboration.
