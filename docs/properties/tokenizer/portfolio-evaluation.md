# Tokenizer portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was run
at U3 by a fresh-context evaluator that had not seen the discovery reasoning: it
was given `../METHOD.md`, `catalog.md`, `existing-checks.md`, `fault-map.md`,
and the crate sources, and was told not to open this file or anything under
`evidence/`. It counted the golden (46 cases, 2137 ids, 606 distinct) and the
asset (64,995 lines) itself and ran `cargo test -p tokenizer` (12 tests, pass).
Its findings are reproduced below with its citations; the disposition is ours.

Four lenses were applied: harness fit, coverage balance, implementability, and a
wildcard pass that questioned the framing itself.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 4 | applied to the catalog |
| gap | 5 | four closed on 2026-09-05 by new records (gaps 1, 3, 4, 5); gap 2 still queued |
| bias | 3 | require human judgment, listed below |

## Refinements applied

1. **`tokenizer-over-long-pieces-are-chunked-and-bounded`: the Check could not
   fail on the Impact it named.** The Guarantee claimed `O(len *
   MAX_PIECE_BYTES)` and the Impact claimed latency regressing to seconds, but
   the Check was structural and every test derived its input size from the
   constant (`token_golden.rs:117`, `:135`, `:152`), so raising
   `MAX_PIECE_BYTES` (`lib.rs:80`) from 4096 to 1 MiB left all three passing
   while restoring near-quadratic cost. The Check now asserts the cap as a
   literal and a minimum chunk count, names the `char_chunks` decomposition
   (BPE is not compositional across arbitrary splits), and states that the
   latency claim has no structural oracle and needs fault class T5.
2. **Same record: `Exercised: yes` overstated.** The concatenation identity is
   asserted for the ASCII case only (`token_golden.rs:129`); the CJK test
   asserts count agreement and a `>= chars/2` floor (`:137-142`); the
   char-boundary conjunct lives in `char_chunks_respect_boundaries_and_cap`
   (`lib.rs:213-220`), which the record had omitted. Now `partial`, with the
   unit test added to Existing check.
3. **`tokenizer-vocabulary-is-embedded-and-complete`: `Exercised` credited a
   check whose subject is a different artifact.** The generator's three checks
   run on the in-memory rows (`gen-claude-vocab.ts:44-60`) before
   `writeFileSync` at `:68`, so they never observe the committed asset that
   `include_str!` embeds (`lib.rs:48`); a post-write truncation passes the
   generator's history and every Rust test. Now `not yet`, in agreement with
   the fault map's "No".
4. **`tokenizer-encoding-matches-the-independent-oracle`: the Guarantee was
   scoped to the corpus, collapsing the property into its test.** Guarantee and
   Check quantified over "every golden case", which is exactly what
   `encode_ordinary_matches_ai_tokenizer_ids` asserts (`token_golden.rs:24-44`),
   so no input outside the 46 cases could violate the record. The Guarantee now
   ranges over any `&str` whose pieces are at most the cap, the Check names the
   golden as the executable sample, and Confidence says `high` refers to the
   evidence trail, not to domain coverage.

## Gaps queued

1. **The pre-tokenizer pattern has tests but no record.** Closed 2026-09-05:
   `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` added. `CLAUDE_PAT_STR`
   (`lib.rs:65-75`) is a hand-substituted rewrite of `assets/claude.pat`
   because ECMAScript and the `regex` crate define `\s` differently
   (`lib.rs:10-13`); `pattern_is_upstream_with_ecmascript_whitespace` and
   `whitespace_class_matches_ecmascript_not_unicode_white_space`
   (`lib.rs:179-210`) assert it and are attached to nothing. An `always` record
   is due: the runtime pattern equals upstream `pat_str` with `\s`/`\S`
   replaced by the ECMAScript class, and that class contains U+FEFF and
   excludes U+0085.
2. **Unicode-table skew in `\p{L}` and `\p{N}`.** The oracle resolves those
   classes against the JS engine's tables and the crate against
   `regex-syntax`'s; a codepoint assigned in a newer Unicode version splits
   differently, and no corpus case exercises a recently assigned script. Needs a
   record plus a human decision on which Unicode version is authoritative.
3. **Totality of `encode_ordinary`.** Closed 2026-09-05 as
   `tokenizer-encoding-is-total-over-valid-utf8` (the fuzz oracle it names is
   still to be written). Six panic sites: five `expect` calls in
   the loader and pattern init (`lib.rs:95`, `:98`, `:99`, `:103`, `:110`) and
   one on the hot path, the backtrack-limit `expect` at `lib.rs:140`, reachable
   for any input above the cap because the pattern carries a negative
   lookahead (`lib.rs:70-71`). No test drives `find_iter` over a large
   whitespace run (the corpus's longest is 50 spaces,
   `gen-token-golden.ts:79`). `estimate_tokens("")` returns without
   initialising the tokenizer (`lib.rs:159-161`) while `encode_ordinary("")`
   initialises it (`:167-169`), so a corrupt asset panics on one entry point
   and not the other. A fuzz target asserting no panic and
   `estimate_tokens(t) == encode_ordinary(t).len()` is the cheapest oracle in
   the crate.
4. **The BOM divergence is a commitment with a self-referential oracle and no
   record.** Closed 2026-09-05 as `tokenizer-bom-is-its-own-token`, with the
   asset ranks as the independent oracle. `bom_before_newline_is_preserved` computes its expectation by
   calling `encode_ordinary` three times and comparing the composite to the
   concatenation (`token_golden.rs:102-109`), so a consistently wrong BOM
   treatment still passes; unlike the prototype-name case, no patched oracle
   pins it. Needs an `always` record whose Confidence line records the circular
   oracle.
5. **Determinism and shared lazy initialisation.** Closed 2026-09-05 as
   `tokenizer-encoding-is-deterministic-across-calls-and-threads`.
   `deterministic_across_calls`
   and `deterministic_across_threads` (`token_golden.rs:64-95`) assert that
   repeated and concurrent calls agree and that the `OnceLock` tokenizer
   (`lib.rs:83-84`) is shared safely; both are unaudited with no record, and
   parity is asserted per case on a single call, so nothing in the set implies
   determinism.

## Biases requiring human judgment

1. **Is "bit-faithful port" the right contract, or is it count agreement?**
   The crate's own statement of purpose is that Rust and the TypeScript harness
   agree on budget math, naming `Tokenizer(claudeEncoding).encode(text,
   "all").length` (`lib.rs:1-4`), a length. The catalog elevates id equality to
   the primary guarantee, which is strictly stronger. Nothing identifies the
   TypeScript harness or checks Rust against it, and if that harness uses stock
   `ai-tokenizer` the crate deliberately disagrees with it on three input
   classes. Which contract is the portfolio defending?
2. **Correct-over-faithful is a product decision the fixture hides.**
   `gen-token-golden.ts:34-38` hands the tokenizer a null-prototype copy of the
   encoder, so the committed golden records ids no stock caller of
   `ai-tokenizer@1.0.6` produces, and the fixture does not record the stock ids
   alongside. Are the divergences properties to defend, or accepted debt to
   retire when upstream fixes them?
3. **All records are `test-only`, yet every Impact is a production
   consequence.** Reachability rests on one verified basis (no workspace member
   depends on `tokenizer`, `Cargo.toml:9`; `publish = false`, `Cargo.toml:20`),
   yet Impact reads "a session overflows or truncates the context window", which
   is unreachable at HEAD. Should Impact be conditional on a future caller, and
   is a three-record catalog the right investment before that caller exists,
   given that the scope note already commits to re-evaluating every record in
   that wave?

## Verdict

The evaluator's verdict, which we adopt: the set is coherent and its harness fit
is mostly sound; every Check except the vocabulary record's can be implemented
with `token_golden.rs` and `include_str!` as stated, and insisting on the
committed fixture over a live oracle is correctly reflected in the code. The gap
that matters is breadth, not depth: the pre-tokenizer pattern, determinism, the
BOM divergence, and totality of `encode_ordinary` each had claim-bearing tests
and no record, which `METHOD.md:140` says must not happen; all four now have
records. Two records over-claimed and are now corrected. For a crate with no
production caller the set is a starting point: gaps 1, 3, 4, and 5 were cheap,
orphan existing tests, and are closed above; gap 2, Unicode-table skew, remains
queued and needs a human decision on the authoritative Unicode version.
