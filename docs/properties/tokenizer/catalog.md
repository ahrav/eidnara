# Tokenizer property catalog

Records for `crates/tokenizer`, discovered at U3; the source had no catalog for this crate. `index.json` is generated
from this file; the record contract is [`../METHOD.md`](../METHOD.md).

## Provenance and scope

- Discovery at U3 against `host@39e8230`. The crate is a port of the `ai-tokenizer` claude encoding. Its contract is
  bit-faithfulness to that oracle for every pre-token piece of at most `MAX_PIECE_BYTES`, with three deliberate
  exceptions documented in `crates/tokenizer/src/lib.rs` (crate docs): pieces longer than the cap are chunked and may
  differ from the oracle at chunk seams; a pre-token equal to an `Object.prototype` member name that is not a vocabulary
  entry (`valueOf`, `hasOwnProperty`, `isPrototypeOf`, `toLocaleString`, `propertyIsEnumerable`) is encoded as bytes
  where stock `ai-tokenizer@1.0.6` emits a function-valued "token"; and a candidate byte slice starting with a UTF-8
  BOM (`EF BB BF`) is scored with the BOM present where the oracle's `TextDecoder` strips it. The golden corpus pins the
  corrected prototype-name ids by running the oracle with a null-prototype encoder copy (`gen/gen-token-golden.ts`), and
  `bom_before_newline_is_preserved` (`crates/tokenizer/tests/token_golden.rs`) pins the BOM case against the crate's own
  single-character encodings, and `tokenizer-bom-is-its-own-token` pins it against the asset ranks; a campaign must not
  treat either divergence as a regression.
- No workspace crate depends on `tokenizer` and nothing outside `crates/tokenizer` calls `encode_ordinary` or
  `estimate_tokens`; the workspace is `publish = false`. Every record is therefore `test-only` at HEAD. Reclassify to
  `default-production` in the wave that adds the first production caller.

## Index

The `Reaches production` column is derived from each record's `Reachability` and
`Status`: `yes` for `default-production`, `no` for `test-only`, and
`n/a - invalidated` for an invalidated record.

| Slug | Type | Confidence | Reaches production |
| --- | --- | --- | --- |
| [tokenizer-encoding-matches-the-independent-oracle](#tokenizer-encoding-matches-the-independent-oracle) | safety | high | no |
| [tokenizer-vocabulary-is-embedded-and-complete](#tokenizer-vocabulary-is-embedded-and-complete) | safety | medium | no |
| [tokenizer-over-long-pieces-are-chunked-and-bounded](#tokenizer-over-long-pieces-are-chunked-and-bounded) | safety | high | no |
| [tokenizer-pattern-is-upstream-with-ecmascript-whitespace](#tokenizer-pattern-is-upstream-with-ecmascript-whitespace) | safety | high | no |
| [tokenizer-bom-is-its-own-token](#tokenizer-bom-is-its-own-token) | safety | high | no |
| [tokenizer-encoding-is-deterministic-across-calls-and-threads](#tokenizer-encoding-is-deterministic-across-calls-and-threads) | safety | high | no |
| [tokenizer-encoding-is-total-over-valid-utf8](#tokenizer-encoding-is-total-over-valid-utf8) | safety | medium | no |

## Records

### tokenizer-encoding-matches-the-independent-oracle

Type: safety
Reachability: test-only - no workspace crate depends on `tokenizer`; only `crates/tokenizer/tests/token_golden.rs` and the crate's unit tests call `encode_ordinary` and `estimate_tokens`.
Status: active
Exercised: partial - 46 golden cases regenerated from the `ai-tokenizer` oracle pass, and a token-count estimate is checked against them; together they exercise 606 distinct token ids of the 64,995 in the vocabulary, so drift in a rank or pattern branch the corpus never reaches is not detected.
Guarantee: For any `&str` whose pre-token pieces are all at most `MAX_PIECE_BYTES` bytes, `encode_ordinary` produces the same token ids as the null-prototype-patched `ai-tokenizer` claude encoding described in Provenance and scope, and `estimate_tokens` returns their count; the two documented oracle defects are excluded by construction and over-long pieces belong to `tokenizer-over-long-pieces-are-chunked-and-bounded`.
Check: `always` - `encode_ordinary(text) == oracle_ids(text)` and `estimate_tokens(text) == oracle_ids(text).len()` for every text in the domain above. The committed golden is the only oracle sample in the tree, so the executable form is `encode_ordinary(text) == golden.ids` over the 46 cases; parity is asserted against that golden, never against a live stock `ai-tokenizer`, so the prototype-name and BOM corrections cannot register as failures.
Fault/timing angle: A vocabulary or pre-tokenizer divergence that keeps counts equal but changes ids.
Required faults and enabling state: The golden corpus, produced by the oracle and not by the crate.
Confidence: high - [evidence](evidence/tokenizer-encoding-matches-the-independent-oracle.md). High in the evidence trail, not in coverage of the domain: the golden is oracle-produced and the comparison is exact, but it samples 46 texts. The corpus texts that named the predecessor were replaced at U3 and the golden regenerated once with `gen/gen-token-golden.ts` against the oracle.
Existing check: `encode_ordinary_matches_ai_tokenizer_ids`, `estimate_tokens_matches_golden_counts` (`crates/tokenizer/tests/token_golden.rs`); unaudited.
Impact: Budget fitting over- or under-counts and a session overflows or truncates the context window.
Open questions: None.

### tokenizer-vocabulary-is-embedded-and-complete

Type: safety
Reachability: test-only - the asset is embedded with `include_str!` and read on first use, but no workspace crate depends on `tokenizer`, so only the crate's tests reach it.
Status: active
Exercised: not yet - the generator (`gen/gen-claude-vocab.ts:44-60`) rejects duplicate ranks, duplicate token byte sequences, and missing single bytes, but it checks the rows it is about to write, before `writeFileSync` at `:68`, never the committed asset that `include_str!` embeds; a post-write truncation or hand edit passes the generator's history and every Rust test. No Rust check parses `assets/claude.tiktoken`.
Guarantee: The embedded `claude.tiktoken` asset has unique ranks, unique token byte sequences, and covers every single byte, so every input encodes without a fallback and every byte sequence maps to exactly one rank.
Check: `always` - ranks are unique, decoded token byte sequences are unique, and 256 single-byte tokens exist in the asset. Byte-sequence uniqueness is load-bearing on the Rust side: `tokenizer()` (`crates/tokenizer/src/lib.rs`) inserts each decoded token into an `FxHashMap<Vec<u8>, Rank>`, so a duplicated sequence under a new rank would silently replace the earlier rank and change encoded ids while rank uniqueness and single-byte coverage still held.
Fault/timing angle: A truncated asset makes some bytes unencodable; a duplicated token sequence changes ids silently.
Required faults and enabling state: None; static asset check.
Confidence: medium - [evidence](evidence/tokenizer-vocabulary-is-embedded-and-complete.md). `gen/gen-claude-vocab.ts` enforces all three conditions when it writes the asset (duplicate ranks and duplicate token byte sequences each abort the write); the Rust side trusts the asset.
Existing check: The generator's checks; no Rust check; unaudited.
Impact: Encoding panics or silently drops bytes, or a duplicated token silently changes ids.
Open questions: None.

### tokenizer-over-long-pieces-are-chunked-and-bounded

Type: safety
Reachability: test-only - every call to `encode_ordinary` and `estimate_tokens` goes through `encode_bounded`, but no workspace crate depends on `tokenizer`, so only the crate's tests reach it.
Status: active
Exercised: partial - the concatenation identity is asserted for an ASCII letter run three caps long; the CJK run above the cap asserts only count agreement and a plausibility floor; the char-boundary conjunct is asserted by `char_chunks_respect_boundaries_and_cap` at unit level; a long text with no over-long piece asserts the unchunked path. Every test derives its input size from `MAX_PIECE_BYTES` itself, so raising the constant leaves all of them passing.
Guarantee: `MAX_PIECE_BYTES` is 4096, and a pre-token piece longer than it is split by `char_chunks` (greedy, at most `MAX_PIECE_BYTES` bytes per chunk, each boundary a `char` boundary) before BPE merging, so merge work is bounded by the cap rather than by the piece; spans with no over-long piece encode exactly as the oracle does, and the ids of a chunked piece equal the concatenation of the `char_chunks` chunks' ids, which may differ from the oracle only at chunk seams.
Check: `always` - `MAX_PIECE_BYTES == 4096` as a literal; for an over-long piece, `encode_ordinary(text)` equals the concatenation of `encode_ordinary` over the prefix, each `char_chunks(piece, MAX_PIECE_BYTES)` chunk, and the suffix, with at least `ceil(piece.len() / MAX_PIECE_BYTES)` chunks; every chunk boundary is a `char` boundary; for text with no over-long piece, `encode_ordinary(text)` equals the piece-by-piece encoding; and `estimate_tokens(text) == encode_ordinary(text).len()` in every case. The decomposition is named because BPE is not compositional across arbitrary splits; only the `char_chunks` split makes the equality true. The asymptotic and latency claim has no structural oracle; it needs the time-bounded check the fault map queues as T5.
Fault/timing angle: A change that removes or raises the cap restores tiktoken's quadratic merge loop, so one long unpunctuated run (37k CJK characters) takes seconds; a change that chunks by byte splits a multi-byte character and panics or corrupts ids.
Required faults and enabling state: An input whose pre-token piece exceeds `MAX_PIECE_BYTES`; the ` ?\p{L}+` pattern makes any long letter run such a piece.
Confidence: high - [evidence](evidence/tokenizer-over-long-pieces-are-chunked-and-bounded.md). `encode_bounded` (`crates/tokenizer/src/lib.rs:133-152`) and `char_chunks` (`:114-128`) were read directly; the tests construct the over-long, multi-byte, and unaffected cases, but none pins the cap's value or measures time, which is why Exercised is partial.
Existing check: `over_long_piece_is_chunked_and_bounded`, `over_long_cjk_piece_keeps_char_boundaries`, `long_text_without_over_long_piece_is_unaffected_by_bound` (`crates/tokenizer/tests/token_golden.rs`) and `char_chunks_respect_boundaries_and_cap` (`crates/tokenizer/src/lib.rs:213`); unaudited.
Impact: Worst-case encoding latency regresses from bounded to seconds, or a campaign demands oracle equality for a long unpunctuated input that the crate deliberately does not promise.
Open questions:

- The three tests derive their input sizes from `MAX_PIECE_BYTES`, so raising the cap to a value that restores near-quadratic cost leaves them green. Should the cap be asserted as a literal and a time-bounded check added, or should the latency claim move to its own record with T5 as its oracle?

### tokenizer-pattern-is-upstream-with-ecmascript-whitespace

Type: safety
Reachability: test-only - `CLAUDE_PAT_STR` (`crates/tokenizer/src/lib.rs:65-75`) is compiled into every build and is the pre-tokenizer for every `encode_ordinary` call, but no workspace crate depends on `tokenizer`, so only the crate's tests reach it; the label moves with the other three records when a production caller lands.
Status: active
Exercised: yes - `pattern_is_upstream_with_ecmascript_whitespace` (`lib.rs:179`) derives the constant from `assets/claude.pat` by substituting the ECMAScript class for `\s` and `\S` and asserts equality plus the absence of any unexpanded `\s` or `\S`; `whitespace_class_matches_ecmascript_not_unicode_white_space` (`:192`) asserts the class matches the sixteen code points it enumerates, U+FEFF among them, and rejects U+0085, U+200B, U+180E, and `a`.
Guarantee: The runtime pre-tokenizer pattern equals upstream `pat_str` with `\s` and `\S` replaced by the ECMAScript whitespace class, and that class follows the ECMAScript definition (U+FEFF included, U+0085 excluded), so piece boundaries match the JavaScript reference rather than the `regex` crate's Unicode `White_Space` definition (`lib.rs:10-13`).
Check: `always` - the derived pattern equals `CLAUDE_PAT_STR`; the constant contains no `\s` or `\S`; the class matches each enumerated ECMAScript code point and none of U+0085, U+200B, U+180E.
Fault/timing angle: An edit to `assets/claude.pat`, to `ecmascript_whitespace!`, or a change in the `regex` crate's class semantics moves piece boundaries and therefore ids while counts stay plausible; the difference surfaces only on inputs containing the code points where the two definitions disagree.
Required faults and enabling state: None; static checks over the constant and the class.
Confidence: high - [evidence](evidence/tokenizer-pattern-is-upstream-with-ecmascript-whitespace.md). The constant, the crate docs at `lib.rs:10-13`, and both tests were read directly.
Existing check: The two tests named above; unaudited.
Impact: A pattern drift silently changes ids for whitespace-adjacent text while `tokenizer-encoding-matches-the-independent-oracle` passes on every golden case that lacks the affected code points. U+FEFF is reached by `bom_before_newline_is_preserved` (`tests/token_golden.rs:101`) and U+0085 by the `nel-after-space` and `nel-runs` golden cases (`gen/gen-token-golden.ts:99-100`, present in `testdata/token-golden.json`), so a class drift on either code point also fails parity.
Open questions: None.

### tokenizer-bom-is-its-own-token

Type: safety
Reachability: test-only - the BOM path is inside `encode_ordinary` for every caller, but no workspace crate depends on `tokenizer`, so only the crate's tests reach it; the label moves with the other records when a production caller lands.
Status: active
Exercised: partial - `bom_before_newline_is_preserved` (`tests/token_golden.rs:101-109`) encodes `"x"`, `"\u{feff}"`, and `"\n"` separately and asserts the composite equals their concatenation, so it proves the BOM is one standalone piece but derives every expected id from `encode_ordinary` itself; a vocabulary edit that moves the BOM's rank passes it.
Guarantee: `encode_ordinary("x\u{feff}\n")` is `[92, 57538, 203]`: U+FEFF (`EF BB BF`, base64 `77u/`) is a standalone token whose id is its asset rank (`assets/claude.tiktoken:57534`), between `x` (`eA==`, rank 92, `:88`) and `\n` (`Cg==`, rank 203, `:199`). The stock oracle's `[92, 203]` is the documented divergence, not the target.
Check: `always` - the three ids equal the ranks read from the asset independently of `encode_ordinary`, and the count is three.
Fault/timing angle: A BOM-stripping decode on the rank-lookup path (the oracle's defect) drops the middle id; a vocabulary edit that merges `EF BB BF` with a neighbour or renumbers it changes the id while the self-referential test still passes because all three single encodings move together.
Required faults and enabling state: None; static check against the asset.
Confidence: high - [evidence](evidence/tokenizer-bom-is-its-own-token.md). The test, the crate docs on the divergence, and the three asset rows were read directly.
Existing check: `bom_before_newline_is_preserved`; unaudited; its oracle is circular, which is the gap this record closes.
Impact: Files that begin with a BOM count one token short or encode a different id; the parity record does not cover this input because the oracle is wrong on it.
Open questions: Should the golden generator pin `[92, 57538, 203]` through a patched oracle, as it already does for prototype-name inputs, so the divergence is asserted in the corpus rather than in a hand-written test? (needs human input)

### tokenizer-encoding-is-deterministic-across-calls-and-threads

Type: safety
Reachability: test-only - the `OnceLock` initialisation of the tokenizer (`crates/tokenizer/src/lib.rs:82-84`) and the pattern (`:108-109`) runs for every caller, but no workspace crate depends on `tokenizer`; the label moves with the other records when a production caller lands.
Status: active
Exercised: yes - `deterministic_across_calls` (`tests/token_golden.rs:64`) repeats `estimate_tokens` a thousand times on one input; `deterministic_across_threads` (`:73`) has eight scoped threads encode every golden case, compares thread 0 to the golden ids and counts, and compares every other thread to thread 0, so concurrent first callers race the lazy initialisation.
Guarantee: `encode_ordinary` and `estimate_tokens` are pure functions of their input: repeated calls, and concurrent calls including ones that race the first initialisation, return identical ids and counts, and `estimate_tokens(t) == encode_ordinary(t).len()` holds on every call.
Check: `always` - for any input, N sequential calls agree, and K concurrent first callers agree with each other and with a later sequential call.
Fault/timing angle: The hazard is a cache or lazy value initialised outside `OnceLock`, or an initialisation that can observe a half-built table; `OnceLock` runs the initialiser once and blocks other callers, so the current shape holds by construction and the record guards a future cache.
Required faults and enabling state: None; concurrent first callers on a cold process.
Confidence: high - [evidence](evidence/tokenizer-encoding-is-deterministic-across-calls-and-threads.md). Both tests and both `OnceLock` sites were read directly.
Existing check: The two tests named above; unaudited.
Impact: Token budgets computed on different threads or at different times disagree, so a session's context accounting drifts without any input change.
Open questions: None.

### tokenizer-encoding-is-total-over-valid-utf8

Type: safety
Reachability: test-only - the panic site is on the `encode_bounded` path every caller takes for inputs above the cap, but no workspace crate depends on `tokenizer`; the label moves with the other records when a production caller lands.
Status: active
Exercised: not yet - the over-long tests (`tests/token_golden.rs:116`, `:134`, `:150`) use letter and CJK runs and assert chunking; nothing drives an input shaped to exhaust `fancy_regex`'s backtrack limit, and the corpus's longest whitespace run is 50 spaces.
Guarantee: `encode_ordinary` and `estimate_tokens` return for every `&str` without panicking. In particular an input longer than `MAX_PIECE_BYTES` (`lib.rs:80`, 4096) never reaches the `expect` at `lib.rs:140`, which aborts on a `fancy_regex` backtrack-limit error before chunking runs.
Check: `always` - a fuzz or adversarial-whitespace oracle over inputs above the cap, including long alternations of whitespace and non-whitespace that exercise the negative lookahead in the pattern (`lib.rs:70-71`), asserts no panic and `estimate_tokens(t) == encode_ordinary(t).len()`.
Fault/timing angle: The pattern's `[ws]+(?![^ws])` needs the backtracking engine, and `encode_bounded` iterates `find_iter` over the whole over-cap text (`lib.rs:138-139`) before any chunking; a match error there is unwrapped with `expect`, so the count aborts the caller's thread instead of returning. Inputs at or below the cap go to `CoreBPE::encode_ordinary` whole (`:133-135`), whose own regex path is outside this record.
Required faults and enabling state: None; an input above 4096 bytes shaped to backtrack.
Confidence: medium - [evidence](evidence/tokenizer-encoding-is-total-over-valid-utf8.md). The panic site and the pattern were read directly; whether any input reaches the default backtrack limit under this pattern is not established.
Existing check: None on the error path; the over-long tests cover chunking only.
Impact: One adversarial or unlucky input aborts token counting in the caller's process rather than returning a count.
Open questions: Does any input reach `fancy_regex`'s default backtrack limit under this pattern, or is the `expect` unreachable in practice? A targeted search or fuzz target answers it; until then the record stands on the panic site alone. (needs human input)

## Relationship map

Grouped by shared mechanism, with suspected dominance noted where one property
holding would make another likely to hold. Dominance is a hypothesis, not proof.

- **One embedded table behind every id.**
  `tokenizer-vocabulary-is-embedded-and-complete` is upstream of
  `tokenizer-encoding-matches-the-independent-oracle`: every id the oracle test
  compares is looked up in the embedded asset, so a corrupted or duplicated
  entry that the golden corpus reaches fails parity, and one it does not reach
  passes both records. Parity therefore dominates completeness only over the
  606 ids the corpus exercises; for the rest, completeness is the only record
  that speaks, and it has no Rust check.
- **Parity below the cap, bounded work above it.**
  `tokenizer-encoding-matches-the-independent-oracle` and
  `tokenizer-over-long-pieces-are-chunked-and-bounded` partition the input space
  by pre-token piece length. Below `MAX_PIECE_BYTES` the first record owns the
  ids; above it the second owns the cost bound and permits seam divergence. A
  test that asserted oracle equality for an over-long piece would contradict the
  second record; a test that relaxed parity below the cap would contradict the
  first. Neither dominates the other.
- **Boundaries before merges.**
  `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` is upstream of
  `tokenizer-encoding-matches-the-independent-oracle`: every id the oracle test
  compares is produced by merging within pieces the pattern cut, so a boundary
  drift fails parity on any golden case containing an affected code point and
  passes on the rest. Parity dominates the pattern record only over the code
  points the corpus reaches.
- **Two divergences, two oracles.** `tokenizer-bom-is-its-own-token` and the
  prototype-name cases inside `tokenizer-encoding-matches-the-independent-oracle`
  are the two places the crate is right and the stock oracle is wrong. The
  prototype-name cases are pinned in the corpus through a patched oracle; the
  BOM case is pinned by asset ranks in its own record. Parity says nothing about
  either input.
- **Purity and totality frame the rest.**
  `tokenizer-encoding-is-deterministic-across-calls-and-threads` says the
  functions return the same answer every time;
  `tokenizer-encoding-is-total-over-valid-utf8` says they return at all. Every
  other record assumes both: a parity or chunking assertion is vacuous on an
  input that panics, and meaningless if a second call could differ.
- **Reachability moves together.** All seven records are `test-only` for one
  reason, the absence of a production caller, so the wave that adds the first
  caller reclassifies all seven at once and must re-evaluate which golden cases
  are load-bearing for that caller's inputs.
