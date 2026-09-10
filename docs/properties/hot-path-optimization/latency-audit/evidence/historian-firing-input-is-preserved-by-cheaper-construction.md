# historian-firing-input-is-preserved-by-cheaper-construction

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Two firing inputs invite cheaper construction. The chunk snapshot copies every
block's bytes into an owned `String` whose only reader takes the byte length,
and the truncation binary-searches with an uncached tokenizer call per probe.
The wildcard pass asked what each replacement must preserve and found that
the fingerprint string is durable state and that the truncation's probe
sequence, not only its budget, decides the output bytes.

## Evidence trail

- The snapshot is built at [`:417-429`][snap-build]: non-synthetic,
  non-system blocks in `start..=end` become
  [`ChunkSnapshotOwnedItem`][owned-item] with `bytes:
  block.bytes.to_string()`.
- Every reader goes through [`as_item`][as-item]: the builder at
  [`:696-701`][fp-call], the restart path at [`lib.rs:4881-4883`][fp-restart],
  and one test. [`compute_chunk_fingerprint`][fp] formats
  `id:kind:bytes.len()` joined by `|`; its doc says the fingerprint records
  byte lengths rather than content bytes.
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
  [`:692`][trunc-call] to build the compartment prompt.
- The [differential header][diff-header] says token counts are not monotonic
  in prefix length, so the probe sequence must be preserved; the frozen
  [`reference`][diff-ref] mirrors the search over a `Vec<u16>`. Three
  proptests pin byte equality: [production windows][diff-prod] (24 cases,
  budget `1..32_001`), [exact budget][diff-exact] (64 cases, returns the
  input unchanged), and [small windows][diff-small] (192 cases, budget
  `0..320`).
- [`forced_overflow_preserves_existing_truncation_output`][t-golden] pins
  one case from `testdata/historian-chunk-golden.json`;
  [`truncation_uses_marker_and_keeps_multibyte_boundaries`][t-marker] checks
  the marker suffix and scalar boundaries;
  [`chunk_fingerprint_uses_id_kind_and_byte_length`][t-fp] pins the literal.

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
test; none exercises a format change across a restart.

## Investigation log

### Q: May truncation relax to "any prefix within budget plus the marker"?

- Sources examined: [`trunc`][trunc], [`diff-header`][diff-header], the
  three proptests, [`t-golden`][t-golden].
- Findings: The tests pin byte identity with the frozen reference, and the
  header states the non-monotonic reason. A relaxed contract changes the
  historian prompt bytes for the same input and would fail every one of
  those tests as written.
- Missing evidence: A specification decision on whether the historian prompt
  bytes are a preserved output or a budget-bounded one.
- Conclusion: needs human input.

[owned-item]: ../../../../../crates/daemon/src/historian_chunk.rs#L29-L35
[as-item]: ../../../../../crates/daemon/src/historian_chunk.rs#L37-L46
[tok-import]: ../../../../../crates/daemon/src/historian_chunk.rs#L15
[snap-build]: ../../../../../crates/daemon/src/historian_chunk.rs#L417-L429
[trunc-call]: ../../../../../crates/daemon/src/historian_chunk.rs#L692
[fp-call]: ../../../../../crates/daemon/src/historian_chunk.rs#L696-L701
[trunc]: ../../../../../crates/daemon/src/historian_chunk.rs#L742-L777
[t-golden]: ../../../../../crates/daemon/src/historian_chunk.rs#L1749-L1760
[t-marker]: ../../../../../crates/daemon/src/historian_chunk.rs#L1762-L1763
[fp]: ../../../../../crates/daemon/src/historian.rs#L140-L158
[fp-field]: ../../../../../crates/memory-store/src/lib.rs#L588
[fp-verify]: ../../../../../crates/daemon/src/historian.rs#L326-L334
[fp-predicate]: ../../../../../crates/daemon/src/historian.rs#L407-L417
[t-fp]: ../../../../../crates/daemon/src/historian.rs#L4006-L4033
[fp-restart]: ../../../../../crates/daemon/src/lib.rs#L4881-L4883
[diff-header]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L1-L11
[diff-ref]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L13-L58
[diff-prod]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L100-L113
[diff-exact]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L115-L129
[diff-small]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L131-L140
