# projection-raw-tools-exact-and-lexical

## Discovery trigger

RP2.1 U4 requires raw output to survive unchanged and create no embedding job
by default. The wildcard distinguishes exact source bytes from parsed JSON
value equality. The property checks retained output and selected byte spans,
not a new codec implementation or lexical tokenizer.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
No tool-corruption incident or executable projection test was supplied.

- [RP2.1 R2][requirements] keeps raw output lexical-only until measured dense
  value is established. [U4][plan] requires unchanged bytes and no default job.
- [OpenCode decode][opencode] reads string output/error from parsed values and
  builds the tool result after checking completed/error status.
- [Pi result conversion][pi] treats a single plain text part specially and
  otherwise retains structured result blocks with raw native part values.
- [Codec goldens][golden] compare `serde_json::Value` arrays and determinism.
  Those assertions do not compare original JSON envelope whitespace/escapes.
- [Historian source capture][historian] serializes selected CK messages. Its
  [existing atomic raw-publication record][raw-record] is a different guarantee.
- The [workspace][workspace] contains no retrieval crate or raw-tool search
  writer. No durable raw-buffer/span contract exists in the named RP2.1 path.

Reachability: `test-only`. Production tool decoding is real, but it does not
write raw-tool projection rows or enforce their default absence of dense jobs.
The proposed byte capture and local job observer are required to exercise this
record. Existing codec paths are reused as inputs, not claimed as full coverage.

## Failure scenario

A projection normalizes CRLF or whitespace, slices by Unicode scalar index,
or stores a rendered preview instead of the complete raw source buffer. Search
then cites bytes that never occurred at the declared source span.
A second failure admits all lexical rows into the generic embedding queue,
silently embedding raw tools despite the default class policy.

A competing explanation is allowed transport decoding: JSON escape spelling
can change while a tool's text value remains identical. The owner must define
the authoritative source-byte boundary. The test captures there before any
lossy projection step; it must not redefine exactness as whatever a renderer
happens to produce. Full retained raw output and selected search spans remain
separate, so selecting a small span does not authorize discarding the original.

## Timing windows and dependencies

Observe at capture, selection, local commit and readback after reopen.
Check pending job identities in the same local committed snapshot.
No inference runtime is needed to detect an unwanted durable job.
Use both native harness encodings, including error and multipart results.
Canonical redaction/admission remains authoritative and may constrain storage.
The boundary between that policy and raw fidelity needs an explicit owner
decision; this record does not authorize bypassing redaction for exact bytes.
Lexical analyzer transformations are allowed in index terms, not retained raw
payloads. FTS tokenization design is outside RP2.1 discovery scope here.

## What a test must construct

1. Independently retained buffers with CRLF, leading/trailing whitespace,
   multibyte characters and JSON-looking escaped text.
2. Overlapping selected byte spans and repeated equal spans from different
   occurrences, with offsets fixed in the approved byte coordinate system.
3. Multipart and non-text result fixtures plus completed and failed tools.
4. Readback after commit/reopen comparing full raw bytes and exact slices.
5. A source class eligible for dense work beside the raw-tool class so the
   no-tool-job check cannot pass merely because all job submission is broken.
6. The two raw precondition markers in [fault-map](../fault-map.md).

This is a source fidelity and policy claim, not evidence of a current failure.
Current golden checks remain unaudited and retain their own catalog records.

## Investigation log

### Q: Which exact byte boundary covers multipart and redacted input?

- Sources examined: [R2][requirements], [U4][plan], [OpenCode][opencode],
  [Pi conversion][pi], [goldens][golden] and [historian capture][historian].
- Findings: The plan requires exact raw preservation. The codecs operate on
  parsed values; Pi may expose a structured collection rather than one text
  string. Historian capture serializes its own selected CK representation.
- Missing evidence: Authoritative capture boundary, encoding of non-text parts,
  span coordinates and policy for canonical admission/redaction changes.
- Conclusion: needs human input from source-adapter and canonical-policy owners.
  Preserve the documented guarantee as a claim; do not weaken it to value
  equality or assume unredacted persistence is permitted.

[requirements]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L35-L41
[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L151-L156
[opencode]: ../../../../../crates/daemon/src/codec/opencode.rs#L490-L559
[pi]: ../../../../../crates/daemon/src/codec/pi.rs#L846-L917
[golden]: ../../../../../crates/daemon/src/codec/mod.rs#L59-L94
[historian]: ../../../../../crates/daemon/src/historian_chunk.rs#L664-L674
[raw-record]: ../../../daemon/historian/catalog.md#publish-preserves-raw-chunk-messages-atomically
[workspace]: ../../../../../Cargo.toml#L3-L17
