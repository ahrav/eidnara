# tokenizer-encoding-is-deterministic-across-calls-and-threads

## Discovery trigger

Two inventoried tests, `deterministic_across_calls` and
`deterministic_across_threads`, were attached to no record, and per-case parity
on a single call does not imply that later or concurrent calls agree.

## Evidence trail

- `crates/tokenizer/tests/token_golden.rs:64-70`: `deterministic_across_calls`
  computes `estimate_tokens` once and asserts equality across 1000 repeats.
- `token_golden.rs:73-95`: `deterministic_across_threads` spawns eight scoped
  threads, each encoding every golden case with `encode_ordinary` and
  `estimate_tokens`; thread 0 is compared to the golden ids and to
  `ids.len()`, and every other thread is compared to thread 0.
- `crates/tokenizer/src/lib.rs:82-84`: `fn tokenizer() -> &'static CoreBPE`
  over `static TOKENIZER: OnceLock<CoreBPE>`; `:108-109`: `fn piece_regex()`
  over `static REGEX: OnceLock<Regex>`. `OnceLock` runs its initialiser once
  and blocks concurrent callers until it completes.
- No other mutable state exists in the crate's encode path (`encode_bounded`,
  `:132-150`, is a pure function of its arguments).

## Failure scenario

A future cache or lazily built table initialised outside `OnceLock`, or one
whose initialiser can be observed half-built, returns different ids to
different callers.

## Timing windows and dependencies

Concurrent first callers racing the lazy initialisation on a cold process.

## What a test must construct

Repeated calls on one input; concurrent first calls from several threads
compared to each other and to a later sequential call.

## Investigation log

### Q: Is any state outside `OnceLock` involved?

- Sources examined: `lib.rs` encode path.
- Findings: none.
- Missing evidence: none.
- Conclusion: the property holds by construction today; the record guards a
  future cache.
