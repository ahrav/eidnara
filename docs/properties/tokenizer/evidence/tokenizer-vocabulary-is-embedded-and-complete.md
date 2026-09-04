# tokenizer-vocabulary-is-embedded-and-complete

## Discovery trigger

The tokenizer loads `assets/claude.tiktoken` through `include_str!`, so the
embedded vocabulary must contain unique ranks and all 256 single-byte tokens.

## Evidence trail

`crates/tokenizer/gen/gen-claude-vocab.ts` rejects duplicate ranks and requires
all 256 single-byte tokens before writing `assets/claude.tiktoken`. The Rust
library embeds that asset and does not validate it at run time.

## Failure scenario

A truncated or duplicated asset leaves at least one byte unencodable.

## Timing windows and dependencies

None. This is a build-time asset property.

## What a test must construct

Run an independent asset check against the embedded file. Assert unique ranks
and all 256 single-byte tokens.

## Investigation log

### Q: Does Rust validate the embedded asset?

- Sources examined: `crates/tokenizer/gen/gen-claude-vocab.ts`,
  `crates/tokenizer/src/lib.rs`.
- Findings: the generator validates the asset before writing it; Rust trusts the
  embedded file.
- Missing evidence: a Rust or independent asset-completeness check.
- Conclusion: unresolved, needs an independent asset check.
