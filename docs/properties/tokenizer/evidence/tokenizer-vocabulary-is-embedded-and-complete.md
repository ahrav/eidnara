# tokenizer-vocabulary-is-embedded-and-complete

## Discovery trigger

The tokenizer embeds `assets/claude.tiktoken`, decoded at build time into a
blob, so the embedded vocabulary must contain unique ranks and all 256
single-byte tokens and the blob must reproduce it.

## Evidence trail

- `crates/tokenizer/gen/gen-claude-vocab.ts:44-52` rejects duplicate ranks and
  duplicate token byte sequences and requires all 256 single-byte tokens before
  writing `assets/claude.tiktoken`.
- `crates/tokenizer/build.rs:19-45` decodes every row into `$OUT_DIR/vocab.bin`
  (`u32 count`, then `u16 len, u32 rank` records, then the token bytes) and
  panics on a malformed row; `src/lib.rs:58` embeds the blob with
  `include_bytes!`.
- `src/bpe.rs:116-150`: `Vocab::from_blob` parses the blob into the byte, pair,
  short, mid, and long tables through `insert` (`:152-170`) and asserts no
  trailing bytes and full single-byte coverage (`:144-148`).
- `src/vocab_blob_tests.rs:7-38`: `vocab_blob_matches_claude_tiktoken` re-reads
  the committed asset, asserts each rank is unique and below the sentinels
  (`:20-21`), resolves each token through the tier that stores it (`:22-31`),
  and asserts the table sizes sum to the row count (`:34-37`).
- In the source tree the Rust library embedded the asset with `include_str!`
  and built an `FxHashMap<Vec<u8>, Rank>` on first use without validating it;
  that gap is what the record was written against.

## Failure scenario

A truncated or duplicated asset leaves at least one byte unencodable or makes
one row's lookup return another row's rank; a `build.rs` or `from_blob` edit
that drops or reorders a record changes ids while the asset is intact.

## Timing windows and dependencies

None. This is a build-time asset property.

## What a test must construct

An independent asset check against the committed file and the runtime tables:
unique ranks, unique byte sequences, all 256 single-byte tokens, and per-row
resolution. Present as `vocab_blob_matches_claude_tiktoken` plus the
`from_blob` load assertion.

## Investigation log

### Q: Does Rust validate the embedded asset?

- Sources examined: `crates/tokenizer/gen/gen-claude-vocab.ts`,
  `crates/tokenizer/src/lib.rs`.
- Findings: the generator validates the asset before writing it; Rust trusts the
  embedded file.
- Missing evidence: a Rust or independent asset-completeness check.
- Conclusion: unresolved, needs an independent asset check.

### Q: Does Rust validate the embedded asset at HEAD?

- Sources examined: `build.rs`, `src/bpe.rs:116-170`,
  `src/vocab_blob_tests.rs`.
- Findings: the blob test decodes the committed asset independently of
  `build.rs` and checks every row against the live tables; `from_blob` asserts
  coverage at load. Byte-sequence uniqueness is asserted indirectly: a duplicate
  would either overwrite a byte or pair slot, leave two short or mid entries, or
  shrink the `ranks` map, each of which fails a per-row or size assertion.
- Missing evidence: none for the stated guarantee.
- Conclusion: Exercised moves to yes and confidence to high.
