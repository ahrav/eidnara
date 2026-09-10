# embedding-count-authority-is-untruncated

## Discovery trigger

P1 line 83 requires an untruncated in-process count from one verified tokenizer
artifact set. A second tokenizer instance is allowed only from those bytes.
This is a documented claim under test, not an implemented guarantee.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only for this proposed property because no production
RP2.1 exact-count path exists at the pinned revision.

## Evidence trail

- `crates/host-runtime/src/synapse/bundle.rs:63-78` defines TokenizerRefs with
  tokenizer, config, special-token map, and tokenizer config artifacts.
- `bundle.rs:259-278` reads their verified bytes and checks the fingerprint.
- `bundle.rs:652-702` binds these hashes and embedding-space scalars. Model
  name is deliberately excluded; it is not interchangeable with fingerprint.
- `crates/host-runtime/src/synapse/inference.rs:297-318` constructs FastEmbed
  from those buffers with the manifest's maximum length.
- `inference.rs:577-589` explicitly counts after truncation. It can certify
  reaching the model window but cannot distinguish at-limit from over-limit.
- `crates/tokenizer/src/lib.rs:1-8`, `:30-35`, and `:58` describe a separate
  embedded Claude BPE vocabulary, with bounded-piece behavior. It cannot serve
  as the embedding model's exact count authority.
- The [existing fingerprint record](../../../host-runtime/catalog.md#synapse-bundle-fingerprint-covers-every-artifact)
  is reused, not cloned. Its tests do not establish count semantics.

## Failure scenario

An input's full encoding exceeds the model window. Counting through the loaded
inference tokenizer returns only the window length. Preflight accepts it and
inference drops its suffix. The full input hash still matches the request.
A second failure uses a Claude count or separately reopened tokenizer file,
making the count plausible while it belongs to another vocabulary or revision.
Neither failure requires a malformed vector or an artifact hash mismatch.

## Timing windows and dependencies

Counting must bind to the same verified lane that later performs inference.
A second instance must not introduce another file lookup or mutable authority.
Its persistent and transient memory still counts against approved limits.
Special tokens are part of the effective sequence; unrelated batch padding
must not inflate an individual text's exact length. The fixture must establish
that interpretation rather than copying the counter's own output.
No count RPC is authorized by P1 or P2. This is an in-process boundary.

## What a test must construct

Use a verified tokenizer fixture with independently pinned full token IDs.
Include at-limit and one-over inputs under the byte cap, equal prefixes with
different suffixes, Unicode, special-token-looking text, and short/long batch
neighbors. Record the count's artifact digest receipt and returned unit.
Replace the artifact pathname after verification while retaining verified bytes.
Compare repeated counts and batch-independent counts to the fixture sequence.
The marker `search_projection_embedding_full_sequence_straddles_window` observes the oracle
input condition, not whether the implementation silently truncates.
Route fixture and type-boundary decisions to `/testing:test-strategy`.

## Investigation log

### Q: Does the existing private token_count already satisfy the plan?

- Sources examined: `inference.rs:313-318`, `:577-589`; P1 line 83.
- Findings: The count uses the truncating tokenizer and is documented as such.
- Missing evidence: None for the distinction between this count and the claim.
- Conclusion: Resolved. Reuse verified artifacts, not the truncated count.

### Q: What independently certifies exact model token count?

- Sources examined: Tokenizer catalog, Synapse fingerprint/corpus checks, P1.
- Findings: Claude golden IDs belong to a different vocabulary; Synapse corpus
  expectations are vectors rather than full untruncated token sequences.
- Missing evidence: An approved immutable sequence fixture and its special-token
  and padding contract, produced independently of the proposed counter.
- Conclusion: Needs human input. No independent token oracle is invented here.
