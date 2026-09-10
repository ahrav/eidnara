# embedding-input-is-rejected-before-inference

## Discovery trigger

P1 lines 58 and 149 require exact token rejection before inference, including
the boundary-plus-one case. Line 115 requires lexical preservation and missing
dense coverage when exact count or tokenizer identity is unavailable.
Sources, date, and SHA: [source register](../catalog.md#source-register).
This proposed product preflight is test-only: no production RP2.1 path joins
verified untruncated counting to dispatch at the pinned revision.

## Evidence trail

- `crates/host-runtime/src/synapse/protocol.rs:810-817` checks text bytes.
- `protocol.rs:825-842` checks query identity, bytes, and deadline shape.
- `protocol.rs:850-887` checks batch identity, aggregate/per-item bytes, input
  hashes, and duplicate IDs before building the request.
- `crates/host-runtime/src/synapse/mod.rs:344-399` checks in-process item and
  byte limits before calling the engine. It has no exact-token check.
- `crates/host-runtime/src/synapse/inference.rs:344-376` rejects empty and
  zero-token input but relies on the inference tokenizer's truncation window.
- `docs/host-wire-protocol.md:472` explicitly permits silent truncation while
  the hash covers all input bytes. Product preflight is an additional boundary.
- [Existing request validation](../../../host-runtime/catalog.md#synapse-requests-are-validated-before-any-inference)
  owns the current wire constraint checks and is reused without duplication.

## Failure scenario

A text fits the byte ceiling but exceeds the exact model token ceiling.
The byte-only path admits it and produces a vector for a prefix, then marks
the full representation covered. Alternatively, counting fails and a heuristic
fallback submits the input anyway. Both violate the planned product behavior.
A full-input hash cannot reveal the omitted suffix because hashing and model
truncation deliberately cover different extents in the existing wire protocol.

## Timing windows and dependencies

The byte gate should precede expensive counting of rejected oversized input.
The exact-count gate must precede engine invocation for every product input,
including backfill and query embedding. Startup certification itself invokes
the engine, so the assertion needs a post-startup call-count baseline.
An accepted boundary input passes preflight without text rewriting; queue
admission or a later deadline may still reject the otherwise valid attempt.
No existing Synapse wire literal is renamed by this discovery record.

## What a test must construct

Pair byte-boundary cases with token-boundary cases whose byte sizes fit.
Use the real verified counter and an engine-call observer rather than a fake
counter that always returns an expected number. Include identity/count failure
and a valid control input proving the engine boundary is reachable.
Read lexical state independently before and after rejection, and observe no
product completion for that K. Projection owns the coverage-state encoding.
`search_projection_embedding_invalid_input_offered_to_ready_lane` records the offered input
and readiness, not the desired zero-call outcome. Inspect captured engine text
for accepted cases to detect silent preflight trimming or replacement.
Existing DeterministicEngine captures calls and text at
`crates/host-runtime/tests/support/synapse.rs:99-105`.

## Investigation log

### Q: Are current boundary tests already exact-token tests?

- Sources examined: `protocol.rs:810-887`, `mod.rs:344-399`, P1 U3.
- Findings: Current admission measures bytes and rows. The only private token
  count is truncated. Matching current boundary tests cannot certify P1 U3.
- Missing evidence: The proposed product counter-to-dispatch path.
- Conclusion: Resolved as a new product gap; existing wire checks are reused.

### Q: Which limits and rejection disposition are authoritative?

- Sources examined: P1 lines 115-121; P2 lines 43-45; the wire contract.
- Findings: RP2.9 owns production values. Existing wire truncation is explicit.
- Missing evidence: Approved caps and the product missing-coverage disposition.
- Conclusion: Needs human input. Do not infer a token-to-byte ratio or invent
  an error literal, and do not interpret lexical survival as dense completion.
