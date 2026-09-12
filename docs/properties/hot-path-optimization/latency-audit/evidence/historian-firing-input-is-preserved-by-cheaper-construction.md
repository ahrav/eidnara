# historian-firing-input-is-preserved-by-cheaper-construction

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
Construction characterization baseline: `32829851`, 2026-09-12.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

At the discovery baseline, the chunk snapshot copies every block's bytes into
an owned `String` whose only reader takes the byte length. Truncation
binary-searches with an uncached tokenizer call per probe.
The wildcard pass asked what each replacement must preserve and found that
the fingerprint string is durable state and that the truncation's probe
sequence, not only its budget, decides the output bytes.

## Evidence trail

- The [snapshot][snap-build] selects non-synthetic,
  non-system blocks in `start..=end` and creates
  [`ChunkSnapshotOwnedItem`][owned-item] entries with owned ID/kind strings
  and `byte_len: block.bytes.len()`. No content string or content handle
  remains in the snapshot. The chunk renderer still reads borrowed flat
  blocks from its caller's projection.
- Production fingerprint consumers use [`as_item`][as-item]: the builder at
  [fingerprint construction][fp-call] and the [restart path][fp-restart].
  [`compute_chunk_fingerprint`][fp] formats `id:kind:byte_len` joined by `|`.
  The unit is UTF-8 bytes, not UTF-16 units or Unicode scalars. Equal-length
  content edits intentionally preserve this diagnostic fingerprint; the
  exact selected-range identity fence is unchanged.
  The baseline leaves `:` and `|` unescaped. This format is not an injective
  encoding, and these checks make no uniqueness claim. Escaping delimiters
  would change durable fingerprint bytes and is outside this preservation
  change's scope.
- [Boundary construction][boundary-view] borrows block IDs with `Cow` and
  shares each original `Arc<str>` from the projection. This adds no cache,
  global state, or retained-resident-byte allowance.
- The string is stored in
  [`HistorianDurableState.chunk_fingerprint`][fp-field], compared by
  [`verify_chunk_fingerprint`][fp-verify], and rejected by the publish
  predicate at [`:407-417`][fp-predicate] with `FingerprintMismatch` after
  abandoning the matching run.
- [`truncate_historian_input_if_needed`][trunc] returns the input when
  `estimate_tokens(input) <= token_budget`, else binary-searches UTF-16 unit
  positions in `0..=unit_len`, calling the uncached
  [`tokenizer::estimate_tokens`][tok-import] on `prefix + marker` per probe,
  and returns the best prefix plus the marker. It is called at
  [prompt assembly][trunc-call] to build the compartment prompt.
- The [differential header][diff-header] says token counts are not monotonic
  in prefix length, so the probe sequence must be preserved; the frozen
  [`reference`][diff-ref] mirrors the search over a `Vec<u16>`. Three
  proptests pin byte equality: [production windows][diff-prod] (24 cases,
  budget `1..32_001`), [exact budget][diff-exact] (64 cases, returns the
  input unchanged), and [small windows][diff-small] (192 cases, budget
  `0..320`).
- [`historian_chunk_golden_fixture_matches_builder`][t-golden] pins
  every truncation case from `testdata/historian-chunk-golden.json` and asserts
  that each input exceeds its budget before comparing exact output;
  [`truncation_uses_marker_and_keeps_multibyte_boundaries`][t-marker] checks
  the marker suffix and scalar boundaries;
  [`chunk_fingerprint_uses_id_kind_and_byte_length`][t-fp] pins the literal.
- The [construction corpus][construction-corpus] compares an owned boundary
  reference with production construction, including pointer identity for
  borrowed IDs and shared original bytes. Its copied-string snapshot oracle
  preserves the source filter and fingerprint format. Frozen transcript
  literals feed the unchanged prompt renderer for exact byte comparison;
  full-prompt digests also pin the shared rendering dependencies. Budgets
  1, 128, and 32,000 cover oversized-first-block truncation and whole chunks.
  The same corpus exercises the [frozen-size lookup][frozen-lookup] with a
  borrowed projected ID and its matching pending drop. An independent
  percentage formula and a no-frozen-entry control make the lookup hit
  observable; the owned reference returns the same exact percentage.
- The [scripted producer capture][firing-capture] checks actual handler
  prompts, not only reconstructed assembly. Both cached-prefix and
  reconstructed-prefix lanes compare full prompt bytes. Their first and
  third prompts also match digests captured before the construction edit.

## Failure scenario

A snapshot item that carries only a length is fine as long as the fingerprint
string is byte-identical; a format change (a hash, a different separator) that
lands while a firing is in flight across a restart makes the stored string
differ from the recomputed one, and the publish fails with
`FingerprintMismatch` after abandoning the run. A truncation that probes a
different sequence, or uses a cached count that differs from the direct count
at a probe, picks a different `best` and the historian receives different
prompt bytes.

## Timing windows and dependencies

The fingerprint window is a restart between `HistorianDurableState` being
written and the publish predicate reading it; the restart path recomputes
from the current snapshot code. Truncation runs only when a chunk's text
exceeds `token_budget`, which needs a large session.

## What a test must construct

A restart with an in-flight firing under a changed fingerprint format; a
chunk whose text exceeds the budget, compared byte-for-byte against the frozen
reference. The
[wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) include
the fingerprint test, the three differentials, the golden, and the marker
test. The construction corpus includes empty messages, combining marks,
supplementary scalars, tools, system and synthetic exclusions, and an excluded
tail. Raw synthetic message shells retain empty projected blocks and the
missing-identity no-fire outcome when included in the selected range. The
producer test covers unflagged synthetic delta replay and normalized cached
prefixes. No test crosses a binary upgrade during an in-flight firing.

## Investigation log

### Q: May truncation relax to "any prefix within budget plus the marker"?

- Sources examined: [`trunc`][trunc], [`diff-header`][diff-header], the
  three proptests, [`t-golden`][t-golden].
- Findings: The tests pin byte identity with the frozen reference, and the
  header states the non-monotonic reason. A relaxed contract changes the
  historian prompt bytes for the same input and would fail every one of
  those tests as written.
- Missing evidence: None for the construction change.
- Conclusion: Exact bytes remain required. No truncation relaxation applies.

### Q: What verification supports the construction change?

- Characterization runs pass before and after the production edit: the
  fingerprint test, construction corpus, scripted producer capture,
  forced-overflow golden, and three truncation differentials.
- All-feature focused runs pass: 140 historian tests (one manual benchmark
  stays ignored), 56 boundary tests, four differential-golden tests, and
  three truncation differentials. These filters overlap.
- Workspace Clippy passes with all targets and all features. The daemon
  release library/binary check and scoped Rust formatting check pass.
- `differential_goldens.rs` and `historian-chunk-golden.json` compare byte
  for byte with `32829851`. The hygiene implementation and `hot_path`
  benchmark source also compare byte for byte with that baseline.
- Limits: No performance run or environment capture is part of this evidence.
  Repository-wide execution and independent reviews remain separate gates.

### Q: Does unflagged replay make the corpus projection stale?

- Sources examined: [normalization][normalize], [apply][apply-normalized],
  [handler observers][handler-observers], [identity exclusion][identity-exclusion],
  and the [assembler's identity check][assembly-identity].
- Findings: `normalize_synthetic_todo_ingress` recognizes reserved task-list
  call/result IDs and marks the pass-local projection without mutating the
  request. `apply_once` projects that normalized view. The handler passes
  the original `parsed` request and `result.projection` together to
  `prepare_historian_fire`, which uses those distinct observers for boundary
  construction and assembly. Synthetic projection entries have no
  `identity_by_mid` row. An unflagged original message inside the selected
  range therefore reaches `MissingBlockIdentity` in the assembler.
- The corpus's `replayed_unflagged` branch models this split, not a pass
  without normalization. It asserts reserved IDs, absent projected identity
  rows, and present but empty boundary message shells. Reprojecting the
  unflagged array without normalization would test a different contract.
- Conclusion: The stale-projection claim is rejected. The no-fire result
  also occurs in the pre-change characterization described below; this is
  not a claim that every handler trigger selects that range.

### Q: Where do the five prompt digests come from?

Provenance: The implementation session transcript on 2026-09-12 contains the
commands and their output. No separate baseline log file was saved. These
runs used the supplied `32829851`-based checkout with test-only patches and
the pre-existing tracker-file edit, before the production construction patch.
They were not clean-worktree runs or cross-binary upgrade tests.

The producer capture command passed and printed both digests in both lanes:

```sh
cargo test -p daemon --locked --lib unflagged_synthetic_delta_prepares_historian_and_native_output -- --nocapture
```

The corpus capture command printed the three retained corpus digests, then
failed on the unflagged, 32,000-token case because its initial test assertion
incorrectly required firing instead of accepting `MissingBlockIdentity`:

```sh
cargo test -p daemon --locked --lib historian_boundary_construction_matches_owned_reference -- --nocapture && cargo test -p daemon --locked --lib chunk_fingerprint_uses_id_kind_and_byte_length
```

After correcting that test expectation and replacing capture prints with
hash assertions, this exact command passed before the production edit:

```sh
cargo test -p daemon --locked --lib historian_boundary_construction_matches_owned_reference && cargo test -p daemon --locked --lib chunk_fingerprint_uses_id_kind_and_byte_length && cargo test -p daemon --locked --lib unflagged_synthetic_delta_prepares_historian_and_native_output
```

| Captured prompt | Expected SHA-256 |
| --- | --- |
| Corpus, budget 1, flagged and unflagged | `38304e8a6ce873573260fbc9967ed890fd10757d9cb9eb49f452b4b551bde4ae` |
| Corpus, budget 128, flagged and unflagged | `f2e94e33234ce5ccb894fe7ca26805cecb16b2d1b775be2a585dc7c9e4876a56` |
| Corpus, budget 32,000, flagged | `aa1ff018eccefed2d25ded6fe08b537bb5cdd43b31dd7012114824a69ac96ce6` |
| Producer, first captured firing in each lane | `f8600b851c98346e9ccede4e9bbdf04b0c1067c3235da0d19a39156248f85776` |
| Producer, third transform's firing in each lane | `3dff45d4a9291a345afccf1f2251d2a860993ac1a84b95165e7845467ed5f882` |

The transcript also contains an earlier corpus variant whose final reply
occupied ordinal 6. Its different whole-chunk digest is not an oracle for
the retained fixture, whose final reply occupies ordinal 8. The table lists
only the five values asserted by the retained tests.

### Q: What does the focused review follow-up verify?

- Eight selected unit tests pass with `--all-features`: the construction
  corpus, fingerprint literal, scripted producer capture, synthetic ingress
  flagged reference, forced overflow, frozen-replacement pressure, relative
  drop target, and production-shape trigger reuse. All three historian
  truncation differentials also pass. A function-name filter initially
  selected zero tests; the concrete pressure and trigger test names above
  were then run and are the counted evidence.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
  passes. Scoped `rustfmt --edition 2024 --config skip_children=true --check`
  passes for `crates/daemon/src/lib.rs` and `crates/daemon/src/historian.rs`.
  Verification commands use 900,000 ms timeouts.
- GitHub content reads at `32829851` confirm 23 changed production-source
  lines across the four production modules, excluding unit tests. The
  follow-up changes only tests and property documentation, not production
  behavior. It does not rerun or supersede the controller's fourteen-gate
  and six-review report.

[owned-item]: ../../../../../crates/daemon/src/historian_chunk.rs#L30-L36
[as-item]: ../../../../../crates/daemon/src/historian_chunk.rs#L40-L46
[tok-import]: ../../../../../crates/daemon/src/historian_chunk.rs#L15
[snap-build]: ../../../../../crates/daemon/src/historian_chunk.rs#L418-L430
[trunc-call]: ../../../../../crates/daemon/src/historian_chunk.rs#L693
[fp-call]: ../../../../../crates/daemon/src/historian_chunk.rs#L697-L702
[trunc]: ../../../../../crates/daemon/src/historian_chunk.rs#L744-L777
[t-golden]: ../../../../../crates/daemon/src/historian_chunk.rs#L1773-L1859
[t-marker]: ../../../../../crates/daemon/src/historian_chunk.rs#L1744-L1757
[boundary-view]: ../../../../../crates/daemon/src/lib.rs#L16656-L16716
[construction-corpus]: ../../../../../crates/daemon/src/lib.rs#L17594
[firing-capture]: ../../../../../crates/daemon/src/lib.rs#L23655
[frozen-lookup]: ../../../../../crates/daemon/src/lib.rs#L16763-L16821
[normalize]: ../../../../../crates/daemon/src/transform.rs#L2129-L2145
[apply-normalized]: ../../../../../crates/daemon/src/transform.rs#L2898-L2911
[handler-observers]: ../../../../../crates/daemon/src/lib.rs#L8354-L8366
[identity-exclusion]: ../../../../../crates/daemon/src/wire.rs#L615-L617
[assembly-identity]: ../../../../../crates/daemon/src/historian_chunk.rs#L646-L658
[fp]: ../../../../../crates/daemon/src/historian.rs#L140-L158
[fp-field]: ../../../../../crates/memory-store/src/lib.rs#L582
[fp-verify]: ../../../../../crates/daemon/src/historian.rs#L326-L334
[fp-predicate]: ../../../../../crates/daemon/src/historian.rs#L407-L417
[t-fp]: ../../../../../crates/daemon/src/historian.rs#L3925
[fp-restart]: ../../../../../crates/daemon/src/lib.rs#L4927-L4929
[diff-header]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L1-L11
[diff-ref]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L13-L58
[diff-prod]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L100-L113
[diff-exact]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L115-L129
[diff-small]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L131-L140
