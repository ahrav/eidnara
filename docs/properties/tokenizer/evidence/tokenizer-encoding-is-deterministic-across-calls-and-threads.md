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

### Q: Is the thread test a cold-start race, and is the pattern OnceLock raced?

- Sources examined: `tests/token_golden.rs:64-95`, `:116`, `:134`, `:150`;
  `lib.rs:82-84`, `:108-110`, `:134-136`; `testdata/token-golden.json` text
  lengths.
- Findings: the eight tests run concurrently in one binary, so any sibling can
  initialise `TOKENIZER` before the thread test spawns, and `load_golden()`
  (`:74`) parses the fixture first; the largest golden text is 3,242 bytes, so
  every call in the thread test takes the early return at `lib.rs:134-136` and
  `piece_regex` is never called there. Three other tests initialise
  `piece_regex` with inputs of 12 KB and above.
- Missing evidence: an isolated cold process with concurrent first callers whose
  inputs include an over-cap string.
- Conclusion: Exercised is partial; the Check names the isolated cold process.
