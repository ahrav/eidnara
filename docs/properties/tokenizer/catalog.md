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
- Re-derived against the post-merge HEAD, in which the crate no longer runs a regex or `tiktoken-rs` at runtime. The
  pre-tokenizer is the hand-written scanner `scan::pieces` (`crates/tokenizer/src/scan.rs:252`), the merge loop is the
  in-crate `bpe::Vocab` (`src/bpe.rs`), and the vocabulary is decoded by `build.rs` into a blob embedded with
  `include_bytes!` (`src/lib.rs:58`). `fancy-regex` and `tiktoken-rs` survive as dev-dependencies behind
  `src/reference_impl.rs` (`#[cfg(test)]`, `lib.rs:42-43`), the port of the earlier implementation that
  `src/parity_tests.rs` compares the live crate against on random inputs. That reference shares the asset and the
  pattern with the live crate, so it is an independent implementation of the same specification, not the `ai-tokenizer`
  oracle; only the golden corpus is oracle-produced.

## Index

The `Reaches production` column is derived from each record's `Reachability` and
`Status`: `yes` for `default-production`, `no` for `test-only`, and
`n/a - invalidated` for an invalidated record.

| Slug | Type | Confidence | Reaches production |
| --- | --- | --- | --- |
| [tokenizer-encoding-matches-the-independent-oracle](#tokenizer-encoding-matches-the-independent-oracle) | safety | high | no |
| [tokenizer-vocabulary-is-embedded-and-complete](#tokenizer-vocabulary-is-embedded-and-complete) | safety | high | no |
| [tokenizer-over-long-pieces-are-chunked-and-bounded](#tokenizer-over-long-pieces-are-chunked-and-bounded) | safety | high | no |
| [tokenizer-pattern-is-upstream-with-ecmascript-whitespace](#tokenizer-pattern-is-upstream-with-ecmascript-whitespace) | safety | high | no |
| [tokenizer-bom-is-its-own-token](#tokenizer-bom-is-its-own-token) | safety | high | no |
| [tokenizer-encoding-is-deterministic-across-calls-and-threads](#tokenizer-encoding-is-deterministic-across-calls-and-threads) | safety | high | no |
| [tokenizer-encoding-is-total-over-valid-utf8](#tokenizer-encoding-is-total-over-valid-utf8) | safety | medium | no |

## Records

### tokenizer-encoding-matches-the-independent-oracle

Type: safety
Reachability: test-only - no workspace crate depends on `tokenizer`; only `crates/tokenizer/tests/token_golden.rs`, `src/parity_tests.rs`, and the crate's other unit tests call `encode_ordinary` and `estimate_tokens`.
Status: active
Exercised: partial - 46 golden cases regenerated from the `ai-tokenizer` oracle pass, and a token-count estimate is checked against them; together they exercise 606 distinct token ids of the 64,995 in the vocabulary, so drift in a rank or pattern branch the corpus never reaches is not detected by the golden. `ids_match_reference_impl` (`src/parity_tests.rs:63`) compares `encode_ordinary` against `reference_impl::encode_ordinary` on 2,000 generated strings per run (`cases()`, `:45-57`; `PROPTEST_CASES` raises it) drawn from ten strategy arms (`text_strategy`, `:11-25`) including arbitrary `String`s, and `count_equals_encode_len` (`:68`) does the same for `estimate_tokens`. That reference is the `tiktoken-rs` `CoreBPE` built from the same asset and driven by the same pattern (`src/reference_impl.rs:37-51`), so it widens coverage of ranks and pattern branches but cannot detect a defect the two implementations share with each other, such as an asset edit; parity with the oracle still rests on the 46 cases.
Guarantee: For any `&str` whose pre-token pieces are all at most `MAX_PIECE_BYTES` bytes, `encode_ordinary` produces the same token ids as the null-prototype-patched `ai-tokenizer` claude encoding described in Provenance and scope, and `estimate_tokens` returns their count; the two documented oracle defects are excluded by construction and over-long pieces belong to `tokenizer-over-long-pieces-are-chunked-and-bounded`.
Check: `always` - `encode_ordinary(text) == oracle_ids(text)` and `estimate_tokens(text) == oracle_ids(text).len()` for every text in the domain above. The committed golden is the only oracle sample in the tree, so the executable form is `encode_ordinary(text) == golden.ids` over the 46 cases; parity is asserted against that golden, never against a live stock `ai-tokenizer`, so the prototype-name and BOM corrections cannot register as failures.
Fault/timing angle: A vocabulary or pre-tokenizer divergence that keeps counts equal but changes ids.
Required faults and enabling state: The golden corpus, produced by the oracle and not by the crate.
Confidence: high - [evidence](evidence/tokenizer-encoding-matches-the-independent-oracle.md). High in the evidence trail, not in coverage of the domain: the golden is oracle-produced and the comparison is exact, but it samples 46 texts. The corpus texts that named the predecessor were replaced at U3 and the golden regenerated once with `gen/gen-token-golden.ts` against the oracle. The reference-implementation parity tests are a second, self-referential line of evidence: they show the scanner and the in-crate BPE reproduce the regex-and-`tiktoken-rs` port on random inputs, which is what makes the 46-case oracle result carry to the rewritten runtime.
Existing check: `encode_ordinary_matches_ai_tokenizer_ids` (`crates/tokenizer/tests/token_golden.rs:24`), `estimate_tokens_matches_golden_counts` (`:47`), `ids_match_reference_impl` and `count_equals_encode_len` (`src/parity_tests.rs:63`, `:68`); unaudited.
Impact: Budget fitting over- or under-counts and a session overflows or truncates the context window.
Open questions: None.

### tokenizer-vocabulary-is-embedded-and-complete

Type: safety
Reachability: test-only - `build.rs` decodes `assets/claude.tiktoken` into `$OUT_DIR/vocab.bin` at build time (`crates/tokenizer/build.rs:19-45`), `lib.rs:58` embeds that blob with `include_bytes!`, and `vocab()` (`lib.rs:95-98`) parses it on first use through `Vocab::from_blob` (`src/bpe.rs:116-150`); no workspace crate depends on `tokenizer`, so only the crate's tests reach it.
Status: active
Exercised: yes - `vocab_blob_matches_claude_tiktoken` (`src/vocab_blob_tests.rs:7-38`) re-reads the committed asset with `include_str!`, decodes every row itself, and asserts per row that the rank is unique and below both sentinels (`:20-21`) and that the live tables return exactly that rank for the token's bytes, by tier: the byte table for one-byte tokens, the pair table for two-byte tokens, a one-piece `encode_piece` for tokens up to 15 bytes, and the `ranks` map above that (`:22-31`); then asserts that the table sizes sum to the row count (`:34-37`), so a row lost to a duplicate byte sequence or a truncated blob fails the count. Single-byte coverage is asserted at load: `from_blob` panics unless every entry of the byte table is set (`bpe.rs:145-148`), and the same test drives that load. The source tree checked these conditions only inside the generator before `writeFileSync`, which is the gap this record was written against.
Guarantee: The embedded `claude.tiktoken` asset has unique ranks, unique token byte sequences, and covers every single byte, and the build-time blob and the runtime tables reproduce it row for row, so every input encodes without a fallback and every byte sequence maps to exactly one rank.
Check: `always` - ranks are unique, decoded token byte sequences are unique, 256 single-byte tokens exist in the asset, and for every asset row the runtime lookup of the token's bytes returns its rank. Byte-sequence uniqueness is load-bearing on the Rust side: `Vocab::insert` (`bpe.rs:152-170`) writes the byte and pair tables by index, so a duplicated one- or two-byte sequence under a new rank would overwrite the earlier rank, and `insert_unique` into the short and mid tables assumes the key is absent; a duplicate there would leave two entries and make the lookup of one row return the other's rank.
Fault/timing angle: A truncated asset makes some bytes unencodable; a duplicated token sequence changes ids silently; a `build.rs` or `from_blob` edit that drops or reorders a record changes ids while the asset itself is intact.
Required faults and enabling state: None; static asset check plus a load of the embedded blob.
Confidence: high - [evidence](evidence/tokenizer-vocabulary-is-embedded-and-complete.md). The generator (`gen/gen-claude-vocab.ts:44-52`) still enforces the conditions when it writes the asset; at HEAD the Rust test above re-asserts them against the committed asset and against the tables the encoder reads, so the asset is no longer trusted.
Existing check: `vocab_blob_matches_claude_tiktoken` (`src/vocab_blob_tests.rs:7`), the `from_blob` coverage assertion (`src/bpe.rs:145-148`), and the generator's checks; unaudited.
Impact: Encoding panics or silently drops bytes, or a duplicated token silently changes ids.
Open questions: None.

### tokenizer-over-long-pieces-are-chunked-and-bounded

Type: safety
Reachability: test-only - every call to `encode_ordinary` and `estimate_tokens` goes through `encode_bounded` (`crates/tokenizer/src/lib.rs:123-142`, called at `:149` and `:155`), but no workspace crate depends on `tokenizer`, so only the crate's tests reach it.
Status: active
Exercised: partial - the concatenation identity is asserted for an ASCII letter run three caps long; the CJK run above the cap asserts only count agreement and a plausibility floor; the char-boundary conjunct is asserted by `char_chunks_respect_boundaries_and_cap` at unit level; a long text with no over-long piece asserts the unchunked path. Every test derives its input size from `MAX_PIECE_BYTES` itself, so raising the constant leaves all of them passing. `ids_match_reference_impl` (`src/parity_tests.rs:63`) also draws letter runs of 3,800 to 4,400 bytes and CJK runs of 3,600 to 4,500 bytes that straddle the cap (`:21-22`) and requires equality with `reference_impl`, whose own `char_chunks` and `MAX_PIECE_BYTES` (`src/reference_impl.rs:35`, `:59`, `:75`) implement the same split, so the seam behaviour is pinned to the reference chunker rather than left to the oracle.
Guarantee: `MAX_PIECE_BYTES` is 4096, and a pre-token piece longer than it is split by `char_chunks` (greedy, at most `MAX_PIECE_BYTES` bytes per chunk, each boundary a `char` boundary) before BPE merging, so merge work is bounded by the cap rather than by the piece; spans with no over-long piece encode exactly as the oracle does, and the ids of a chunked piece equal the concatenation of the `char_chunks` chunks' ids, which may differ from the oracle only at chunk seams.
Check: `always` - `MAX_PIECE_BYTES == 4096` as a literal; for an over-long piece, `encode_ordinary(text)` equals the concatenation of `encode_ordinary` over the prefix, each `char_chunks(piece, MAX_PIECE_BYTES)` chunk, and the suffix, with at least `ceil(piece.len() / MAX_PIECE_BYTES)` chunks; every chunk boundary is a `char` boundary; for text with no over-long piece, `encode_ordinary(text)` equals the piece-by-piece encoding; and `estimate_tokens(text) == encode_ordinary(text).len()` in every case. The decomposition is named because BPE is not compositional across arbitrary splits; only the `char_chunks` split makes the equality true. The asymptotic and latency claim has no structural oracle; it needs the time-bounded check the fault map queues as T5.
Fault/timing angle: A change that removes or raises the cap restores tiktoken's quadratic merge loop, so one long unpunctuated run (37k CJK characters) takes seconds; a change that chunks by byte splits a multi-byte character and panics or corrupts ids.
Required faults and enabling state: An input whose pre-token piece exceeds `MAX_PIECE_BYTES`; the ` ?\p{L}+` alternative, implemented by the scanner's maximal letter run (`src/scan.rs:177-200`), makes any long letter run such a piece.
Confidence: high - [evidence](evidence/tokenizer-over-long-pieces-are-chunked-and-bounded.md). `encode_bounded` (`crates/tokenizer/src/lib.rs:123-142`, the cap test at `:131` and the chunk loop at `:134-138`), `char_chunks` (`:101-115`), and `MAX_PIECE_BYTES` (`:93`) were read directly; the tests construct the over-long, multi-byte, and unaffected cases, but none pins the cap's value or measures time, which is why Exercised is partial. The bound is now also load-bearing inside the merge loop: `merge_scan` stores piece-relative offsets as `u32` on the strength of the cap (`src/bpe.rs:232-234`).
Existing check: `over_long_piece_is_chunked_and_bounded` (`crates/tokenizer/tests/token_golden.rs:116`), `over_long_cjk_piece_keeps_char_boundaries` (`:134`), `long_text_without_over_long_piece_is_unaffected_by_bound` (`:150`), `char_chunks_respect_boundaries_and_cap` (`crates/tokenizer/src/lib.rs:209`), and the straddling arms of `ids_match_reference_impl` (`src/parity_tests.rs:21-22`); unaudited. `engine_crossover` (`src/bpe.rs:425`) is an ignored timing probe, not a bound.
Impact: Worst-case encoding latency regresses from bounded to seconds, or a campaign demands oracle equality for a long unpunctuated input that the crate deliberately does not promise.
Open questions:
- The three tests derive their input sizes from `MAX_PIECE_BYTES`, so raising the cap to a value that restores near-quadratic cost leaves them green. Should the cap be asserted as a literal and a time-bounded check added, or should the latency claim move to its own record with T5 as its oracle?

### tokenizer-pattern-is-upstream-with-ecmascript-whitespace

Type: safety
Reachability: test-only - the runtime pre-tokenizer is the hand-written scanner `scan::pieces` (`crates/tokenizer/src/scan.rs:252-263`), driven by `encode_bounded` for every `encode_ordinary` and `estimate_tokens` call (`src/lib.rs:130`); no regex runs at runtime, and `CLAUDE_PAT_STR` (`lib.rs:77-88`) is `#[cfg(test)]`, present only so the unit tests can tie the scanner back to the upstream pattern. No workspace crate depends on `tokenizer`, so only the crate's tests reach the scanner; the label moves with the other records when a production caller lands.
Status: active
Exercised: partial - the claim now rests on a chain. `pattern_is_upstream_with_ecmascript_whitespace` (`lib.rs:168`) derives the test constant from `assets/claude.pat` by substituting `ecmascript_whitespace!()` for `\s` and `\S` (`:169-174`) and asserts equality plus the absence of any unexpanded `\s` or `\S` (`:175-177`); `reference_pattern_equals_upstream_derived_pattern` (`:183`) asserts that constant equals `reference_impl::CLAUDE_PAT_STR` (`src/reference_impl.rs:23-33`), the pattern the reference actually compiles (`:56`); `matches_reference_on_hand_cases` (`src/scan.rs:302`) compares the scanner's splits against the reference regex on hand-written cases; and `ids_match_reference_impl` (`src/parity_tests.rs:63`) requires id equality with the reference on 2,000 generated strings per run, whose whitespace arm (`:12`, `:17`) draws from all 25 class members plus U+0085 and U+200B, and whose last arm (`:23`) mixes U+FEFF and U+0085 into ordinary text. The runtime class is a literal in `class_from_tables` (`scan.rs:71`) and `ASCII_CLASS` (`:38`), separate from the macro, so a drift between the two fails parity on the next generated input that contains the moved code point; because the reference expands the same macro as `lib.rs`, a wrong edit made to both macros at once passes every check except `whitespace_class_matches_ecmascript_not_unicode_white_space` (`lib.rs:188`), which pins 16 of the 25 members (U+FEFF among them) and rejects U+0085, U+200B, U+180E, and `a`; U+2001 through U+2009 are asserted against nothing but the macro. The `\p{L}` and `\p{N}` classes come from `src/unicode_tables.rs`, pinned to `regex-syntax` by `unicode_tables_match_regex_syntax` (`src/unicode_gen_tests.rs:68`).
Guarantee: The runtime scanner cuts pieces exactly where upstream `pat_str` with `\s` and `\S` replaced by the ECMAScript whitespace class would cut them under leftmost-first alternation, and that class follows the ECMAScript definition (U+FEFF included, U+0085 excluded), so piece boundaries match the JavaScript reference rather than the `regex` crate's Unicode `White_Space` definition (`lib.rs:10-13`).
Check: `always` - the derived pattern equals `CLAUDE_PAT_STR` and the reference's constant; neither contains `\s` or `\S`; `scan::pieces(text)` equals the reference regex's match spans for every text; and the class the scanner uses matches exactly the 25 ECMAScript WhiteSpace and LineTerminator code points (U+0009, U+000A, U+000B, U+000C, U+000D, U+0020, U+00A0, U+1680, U+2000 through U+200A, U+2028, U+2029, U+202F, U+205F, U+3000, U+FEFF), stated as a literal set independent of the macro, and none of U+0085, U+200B, U+180E.
Fault/timing angle: An edit to `assets/claude.pat`, to either `ecmascript_whitespace!` macro, to the class literals in `scan.rs`, to the scanner's run or give-back logic (`scan.rs:239-250`), or a `unicode_tables.rs` regeneration against a different Unicode version moves piece boundaries and therefore ids while counts stay plausible; the difference surfaces only on inputs containing the affected code points, and a drift shared by the scanner and the reference is invisible to parity.
Required faults and enabling state: None; static checks over the constants and the class, plus generated inputs containing the disputed code points.
Confidence: high - [evidence](evidence/tokenizer-pattern-is-upstream-with-ecmascript-whitespace.md). The scanner, its class tables, the test constant, the reference constant, the crate docs at `lib.rs:10-13`, and the five tests named above were read directly. The source tree compiled `CLAUDE_PAT_STR` into the runtime and this record described that constant as the pre-tokenizer; at HEAD the constant is test-only and the record's subject is the scanner it is checked against.
Existing check: The five tests named above plus `unicode_tables_match_regex_syntax`; unaudited.
Impact: A pattern drift silently changes ids for whitespace-adjacent text while `tokenizer-encoding-matches-the-independent-oracle` passes on every golden case that lacks the affected code points. U+FEFF is reached by `bom_before_newline_is_preserved` (`tests/token_golden.rs:101`) and by the `zero-width`, `bom-leading`, and `bom-between-punct` golden cases (`gen/gen-token-golden.ts:76`, `:101-102`); U+0085 by `nel-after-space` and `nel-runs` (`:99-100`). U+1680 and U+2000 through U+200A appear in no golden case, so a drift on any of those twelve fails oracle parity nowhere; at HEAD they are reached by the generated whitespace arm of the reference parity test, which catches a scanner-only or macro-only drift but not one made to the scanner and both macros together.
Open questions: None; the nine members asserted only through the macro are a coverage gap queued in `fault-map.md`, not a decision.

### tokenizer-bom-is-its-own-token

Type: safety
Reachability: test-only - the BOM path is inside `encode_ordinary` for every caller, but no workspace crate depends on `tokenizer`, so only the crate's tests reach it; the label moves with the other records when a production caller lands.
Status: active
Exercised: partial - `bom_before_newline_is_preserved` (`tests/token_golden.rs:101-109`) encodes `"x"`, `"\u{feff}"`, and `"\n"` separately and asserts the composite equals their concatenation, so it proves the BOM is one standalone piece but derives every expected id from `encode_ordinary` itself; a vocabulary edit that moves the BOM's rank passes it.
Guarantee: `encode_ordinary("x\u{feff}\n")` is `[92, 57538, 203]`: U+FEFF (`EF BB BF`, base64 `77u/`) is a standalone token whose id is its asset rank (`assets/claude.tiktoken:57534`), between `x` (`eA==`, rank 92, `:88`) and `\n` (`Cg==`, rank 203, `:199`). The stock oracle's `[92, 203]` output for this input is the documented divergence, asserted only in crate and test docs (`lib.rs:25-28`, `tests/token_golden.rs:97-99`); no fixture in the tree pins it.
Check: `always` - the three ids equal the ranks read from the asset independently of `encode_ordinary`, and the count is three.
Fault/timing angle: A BOM-stripping decode on the rank-lookup path (the oracle's defect) drops the middle id; at HEAD the lookup never decodes, because `Vocab::rank_of` (`src/bpe.rs:175-193`) keys the byte, pair, short, mid, and long tables on raw bytes, so the defect can only return through a lookup rewrite. A vocabulary edit that merges `EF BB BF` with a neighbour or renumbers it changes the id while the self-referential test still passes because all three single encodings move together.
Required faults and enabling state: None; static check against the asset.
Confidence: high - [evidence](evidence/tokenizer-bom-is-its-own-token.md). The test, the crate docs on the divergence (`lib.rs:25-28`), the byte-keyed lookup, and the three asset rows were read directly.
Existing check: `bom_before_newline_is_preserved`; unaudited. Its oracle is circular, which matters for the piece shape only: a renumbered `EF BB BF` row is already caught by `encode_ordinary_matches_ai_tokenizer_ids` on the three BOM golden cases, where id 57538 is pinned against the oracle.
Impact: The divergence fires only when the BOM shares one whitespace pre-token piece with a following whitespace character, as in `"x\u{feff}\n"`, where the stock oracle scores the whole piece `EF BB BF 0A` as `"\n"`. A leading BOM followed by a non-whitespace character is unaffected: the oracle-produced `bom-leading` golden case (`gen/gen-token-golden.ts:101`, ids `[57538, 2666, 679, 284, 355, 31]`) carries the BOM token, as do `zero-width` (`:76`) and `bom-between-punct` (`:102`). The record's residue is therefore the BOM-plus-adjacent-whitespace piece shape, which parity cannot cover because the oracle is wrong on exactly that input.
Open questions: Should the golden generator pin `[92, 57538, 203]` through a patched oracle, as it already does for prototype-name inputs, so the divergence is asserted in the corpus rather than in a hand-written test? (needs human input)

### tokenizer-encoding-is-deterministic-across-calls-and-threads

Type: safety
Reachability: test-only - the one `OnceLock` left in the runtime, `vocab()` (`crates/tokenizer/src/lib.rs:95-98`), runs for every caller; the scanner's class tables are compile-time constants (`src/scan.rs:29`, `:82`) and the pattern `OnceLock` the source tree initialised for over-cap inputs no longer exists. No workspace crate depends on `tokenizer`; the label moves when a production caller lands.
Status: active
Exercised: partial - `deterministic_across_calls` (`tests/token_golden.rs:64`) repeats `estimate_tokens` a thousand times on one input; `deterministic_across_threads` (`:73`) has eight scoped threads encode every golden case, compares thread 0 to the golden ids and counts, and compares every other thread to thread 0; `encode_is_pure_across_threads` (`src/parity_tests.rs:95-118`) computes 2,000 expected encodings on the calling thread and has eight scoped threads re-encode all of them, which covers shared-state safety after initialisation and, because the `Scratch` buffers are per call (`lib.rs:128`), the absence of any cross-call state. The cold-start race is schedule-dependent: the tests in each binary run concurrently, any sibling can initialise `VOCAB` first, and `load_golden()` (`:74`) parses the fixture before the threads spawn.
Guarantee: `encode_ordinary` and `estimate_tokens` are pure functions of their input: repeated calls, and concurrent calls including ones that race the first initialisation, return identical ids and counts, and `estimate_tokens(t) == encode_ordinary(t).len()` holds on every call.
Check: `always` - in an isolated cold process, K concurrent first callers agree with each other and with a later sequential call, and N sequential calls on any input agree. The conjunct `estimate_tokens(t) == encode_ordinary(t).len()` holds by construction (`lib.rs:148-156`, both return through `encode_bounded`) and is a screen, not evidence; `count_equals_encode_len` (`src/parity_tests.rs:68`) asserts it anyway.
Fault/timing angle: The hazard is a cache or lazy value initialised outside `OnceLock`, or an initialisation that can observe a half-built table; `OnceLock` runs the initialiser once and blocks other callers, and `from_blob` builds the whole `Vocab` before returning it (`src/bpe.rs:116-150`), so the current shape holds by construction and the record guards a future cache. The engine choice inside `encode_piece` (`bpe.rs:219-229`) depends only on the piece's bytes, and `heap_and_scan_engines_agree` (`bpe.rs:381`) asserts the two engines produce the same ids, so the threshold cannot make a result depend on anything but the input.
Required faults and enabling state: None; concurrent first callers on a cold process.
Confidence: high - [evidence](evidence/tokenizer-encoding-is-deterministic-across-calls-and-threads.md). The three tests, the single `OnceLock` site, and the engine selection were read directly.
Existing check: The three tests named above and `heap_and_scan_engines_agree`; unaudited.
Impact: Token budgets computed on different threads or at different times disagree, so a session's context accounting drifts without any input change.
Open questions: None.

### tokenizer-encoding-is-total-over-valid-utf8

Type: safety
Reachability: test-only - every caller takes the `encode_bounded` path (`crates/tokenizer/src/lib.rs:123-142`), but no workspace crate depends on `tokenizer`; the label moves with the other records when a production caller lands.
Status: active
Exercised: partial - `ids_match_reference_impl` and `count_equals_encode_len` (`src/parity_tests.rs:63`, `:68`) run `encode_ordinary` and `estimate_tokens` on 2,000 generated strings per run, one arm of which is `any::<String>()` (`:20`) and another mixes combining marks, emoji, and Arabic with whitespace (`:23`), and a panic on any of them fails the property; `vocab_blob_matches_claude_tiktoken` (`src/vocab_blob_tests.rs:7`) drives `from_blob`'s load-time assertions. Nothing targets the scanner's index arithmetic or the merge loops with a fuzzer, and the generated strings are at most a few thousand bytes.
Guarantee: `encode_ordinary` and `estimate_tokens` return for every `&str` without panicking. In particular `scan::pieces` yields spans that cover the text exactly, in order, on `char` boundaries, and `Vocab::encode_piece` returns for every such span of 1 to `MAX_PIECE_BYTES` bytes.
Check: `always` - a fuzz or adversarial oracle over arbitrary `&str`, including long alternations of whitespace and non-whitespace, pieces at exactly the cap, and every class boundary the scanner distinguishes, asserts no panic, that the spans of `scan::pieces(t)` tile `t`, and `estimate_tokens(t) == encode_ordinary(t).len()`.
Fault/timing angle: The source tree's panic site was an `expect` on `fancy_regex`'s backtrack-limit error inside `encode_bounded`, reachable in principle for any over-cap input shaped to backtrack; the rewrite removed the regex, so that site is gone, and the record now guards the replacements: the scanner indexes `bytes[pos]`, `bytes[pos + 1]`, and `bytes[pos + 2]` behind explicit bounds tests (`src/scan.rs:205-213`) and walks back over continuation bytes in `whitespace_piece_end` (`:239-250`); `encode_piece` asserts `start < end && end <= text.len()` and `piece.len() >= 2` only in debug builds (`src/bpe.rs:206`, `:218`), so a scanner regression that yields an empty or reversed span reaches the merge loops unchecked in release; `merge_scan` and `merge_heap` index `parts` and the heap by piece-relative offsets. A panic in any of these aborts the caller's thread instead of returning a count.
Required faults and enabling state: None; arbitrary input, with emphasis on class boundaries, contraction prefixes, and pieces at the cap.
Confidence: medium - [evidence](evidence/tokenizer-encoding-is-total-over-valid-utf8.md). The scanner and both merge engines were read directly and the proptest arms exercise them on random input; no fuzz target or adversarial generator exists, and the debug-only assertions mean the release build has no in-process guard, which is why this stays medium.
Existing check: The two parity properties and the blob test named above; unaudited. None targets a panic path.
Impact: One adversarial or unlucky input aborts token counting in the caller's process rather than returning a count.
Open questions: Should a `cargo fuzz` target over `encode_ordinary` replace the proptest arms as this record's oracle, and should the two `debug_assert!`s in `encode_piece` become release assertions so a scanner regression fails loudly at the seam rather than inside the merge loop? (needs human input)

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
  that speaks, and `vocab_blob_matches_claude_tiktoken` is its check.
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
- **One reference, shared blind spots.** At HEAD every record except the golden
  parity leans on `src/reference_impl.rs`: the pattern record's chain ends at
  the reference pattern, the chunking record's seam behaviour is pinned to the
  reference chunker, and the totality and determinism records borrow the parity
  properties' random inputs. The reference decodes the same asset and expands
  the same whitespace macro as the live crate, so an asset edit, a macro edit
  made to both copies, or a cap change made to both constants passes every
  parity property; only the 46 oracle-produced golden cases and the literal
  whitespace test see those. Parity against the reference dominates the other
  records over implementation drift, and nothing else, which is why the golden
  remains the only oracle record.
- **Purity and totality frame the rest.**
  `tokenizer-encoding-is-deterministic-across-calls-and-threads` says the
  functions return the same answer every time;
  `tokenizer-encoding-is-total-over-valid-utf8` says they return at all. Every
  other record assumes both: a parity or chunking assertion is vacuous on an
  input that panics, and meaningless if a second call could differ.
- **Reachability moves, but not as one.** All seven records are `test-only` for
  one reason, the absence of a production caller, but the first caller need not
  reclassify them together: a caller that only calls `estimate_tokens` on short
  text reaches parity, vocabulary, and determinism without ever reaching the
  over-cap path, so the chunking and totality records would stay `test-only`
  while the others become `default-production`. The wave that adds the caller
  must re-evaluate each record against that caller's inputs.
