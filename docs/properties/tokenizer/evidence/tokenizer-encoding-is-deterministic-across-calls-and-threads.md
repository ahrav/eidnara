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
- `src/parity_tests.rs:95-118`: `encode_is_pure_across_threads` encodes 2,000
  texts on the calling thread, then has eight scoped threads re-encode them and
  assert equal ids and counts.
- `crates/tokenizer/src/lib.rs:95-98`: `fn vocab() -> &'static bpe::Vocab` over
  `static VOCAB: OnceLock<bpe::Vocab>`, the only `OnceLock` in the runtime;
  `from_blob` (`src/bpe.rs:116-150`) builds the whole table before returning.
  The scanner's tables are compile-time constants (`src/scan.rs:29`, `:82`).
- No other mutable state exists in the crate's encode path: `encode_bounded`
  (`lib.rs:123-142`) allocates a fresh `Scratch` per call (`:128`), and the
  engine choice in `encode_piece` (`src/bpe.rs:219-229`) depends only on the
  piece's bytes; `heap_and_scan_engines_agree` (`bpe.rs:381`) asserts the two
  engines agree.
- In the source tree there were two `OnceLock`s, the `CoreBPE` and the pattern
  regex, and an early return skipped the pattern for inputs at or below the cap.

## Failure scenario

A future cache or lazily built table initialised outside `OnceLock`, or one
whose initialiser can be observed half-built, returns different ids to
different callers.

## Timing windows and dependencies

Concurrent first callers racing the lazy initialisation on a cold process.

## What a test must construct

Repeated calls on one input; concurrent first calls from several threads in an
isolated cold process compared to each other and to a later sequential call.

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

### Q: What changed in the shared state at HEAD?

- Sources examined: `src/lib.rs:95-156`, `src/scan.rs`, `src/bpe.rs:198-230`,
  `src/parity_tests.rs:95-118`.
- Findings: one `OnceLock` remains and it publishes a fully built `Vocab`; the
  pattern `OnceLock` and the over-cap early return are gone, so the Check no
  longer needs an over-cap input to race a second initialiser. Per-call
  `Scratch` and a byte-determined engine choice leave no cross-call state.
- Missing evidence: an isolated cold process; sibling tests initialise `VOCAB`
  first in every binary.
- Conclusion: Exercised stays partial; the Check drops the over-cap conjunct.
