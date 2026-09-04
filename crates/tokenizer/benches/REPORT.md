# Tokenizer autoresearch report

Branch `perf/tokenizer-autoresearch` off `u3/5-tokenizer` (PR #22). Updated after every keep.

## Status

Iterations 0-35 done: 12 keeps, 18 discards (5 measured with the full report, 13 prescreened
with `perf stat` cycles), 1 attribution + 1 re-attribution, 2 A/A controls, 1 build-profile
measurement. Baseline composite **133.58 ns/byte** -> current best **8.73 ns/byte**
(15.3x faster). Per-iteration verdicts, CIs and counters: `results.tsv`; the same rows in
`.auto/log.jsonl`. Open ideas: `.auto/ideas.md`.

## Baseline -> current best

One direct paired run of the Phase-0 binary (commit fb48720, built fresh) against the current
best (commit f54d2b9), 6 KCCK/CKKC blocks on core 8, bootstrap 95% CIs over blocks
(`benches/baseline_vs_current.json`):

| arm | baseline ns/B | current ns/B | ratio [95% CI] | speedup |
|---|---|---|---|---|
| adversarial_long_piece | 1356.51 | 60.47 | 0.0447 [0.0445, 0.0450] | 22.4x |
| ascii_prose | 79.27 | 2.57 | 0.0322 [0.0321, 0.0323] | 31.1x |
| ascii_prose_count | 79.43 | 2.56 | 0.0321 [0.0319, 0.0323] | 31.1x |
| cjk | 56.76 | 16.78 | 0.2960 [0.2955, 0.2964] | 3.4x |
| code | 152.63 | 3.56 | 0.0233 [0.0232, 0.0233] | 43.0x |
| code_count | 152.95 | 3.56 | 0.0232 [0.0230, 0.0233] | 43.2x |
| mixed_unicode | 97.26 | 11.63 | 0.1200 [0.1197, 0.1205] | 8.3x |
| numeric | 179.43 | 12.08 | 0.0671 [0.0669, 0.0672] | 14.9x |
| short_strings | 81.29 | 15.80 | 0.1926 [0.1898, 0.1947] | 5.2x |
| whitespace_heavy | 112.65 | 13.76 | 0.1190 [0.1180, 0.1198] | 8.4x |
| **composite** | **133.58** | **8.73** | **0.0651 [0.0649, 0.0652]** | **15.4x** |
| cold_start (ms, fresh process) | 28.0 | 3.2 | 0.115 | 8.7x |

## Design summary of kept implementation

- Pre-tokenizer (`src/scan.rs`): hand-written scanner over four character classes (letter,
  number, ECMAScript whitespace, other); the `\s+(?!\S)` lookahead is "maximal whitespace run,
  give back the last char if a non-whitespace char follows". ASCII lead bytes take a table
  path; runs of one class advance eight ASCII bytes per step with SWAR class masks
  (exhaustively lane-tested against the byte table) and fall back to per-character
  classification on the first byte >= 0x80. Non-ASCII BMP characters classify through a
  two-bit table computed by `build.rs`; astral characters binary-search the range tables.
  `\p{L}`/`\p{N}` come from `src/unicode_tables.rs`, generated from `regex-syntax 0.8.11`
  (Unicode 16.0.0) and pinned by `unicode_gen_tests::unicode_tables_match_regex_syntax`.
- BPE (`src/bpe.rs`): in-crate, tiktoken's order (lowest rank, leftmost tie). Whole-piece
  lookup first; initial pair ranks from a 256x256 table; two engines chosen by piece length and
  lead byte: the O(n·m) rescan in a reusable scratch buffer up to 192 bytes (40 for pieces
  starting with a non-ASCII byte), and a linked-list + min-heap engine above (O((n+m) log n));
  `bpe::tests::heap_and_scan_engines_agree` checks 20k random pieces across both. The heap key
  is `(rank << 32) | position` in one `u64`, so ties resolve leftmost with a single compare.
- Rank lookup: byte table (1 B), pair table (2 B), `hashbrown::HashTable<(u64, Rank)>` with
  the token bytes packed inline in the key (3..=7 B), the same with `u128` keys (8..=15 B), and
  `FxHashMap<&'static [u8], Rank>` for the ~700 longer tokens. Keys and the borrowed slices
  point into a vocabulary blob that `build.rs` decodes from `assets/claude.tiktoken`; the first
  call builds the tables in one pass (3.2 ms).
- Chunking at `MAX_PIECE_BYTES = 4096` unchanged; the heap engine makes a 4 KiB chunk cost
  ~61 ns/byte instead of ~1350.
- Dependencies: runtime `hashbrown` (default features off; already in the lockfile through
  std) and `rustc-hash`. `base64` moved to build- and dev-dependencies. `tiktoken-rs` (exact
  pin) and `fancy-regex` are dev-dependencies for the reference oracle; `regex-syntax` is a
  dev-dependency for the Unicode table generator and drift test; `proptest` added.
- `unsafe`: none in the crate (`grep -c unsafe src/*.rs` = 0). The one experiment that needed
  a block (iteration 10, `repr(transparent)` key cast) was discarded on its own numbers.

## Evidence for invariants (Phase 0 harness)

- Differential corpus: 24,788 strings (14k arm-distributed 1..600 B, 4k short strings, 4k
  mutation-fuzzed golden cases, 1k golden splices, 2k strings of 4097..8192 B with every piece
  <= MAX_PIECE_BYTES); 212 texts dropped by the exact BOM rule (U+FEFF sharing a piece with
  another character). All ids match `ai-tokenizer@1.0.6` (null-prototype encoder, piecewise
  `encode(piece, "all")`; the reference's whole-text `< 10` chars shortcut is equivalent because
  no vocabulary entry spans a piece boundary, asserted by the generator).
- Property tests vs `src/reference_impl.rs` (frozen tiktoken-rs path): 50,000 cases each for id
  parity, `estimate_tokens == encode_ordinary.len()`, and concat-at-piece-boundary; strategies
  include pieces of 3800..4400 letters and 1200..1500 CJK chars around the chunking cap
  (verified to catch a cap change: setting `MAX_PIECE_BYTES = 4000` fails the guard).
- Thread purity: 8 threads x 2000 strings, plus the golden `deterministic_across_threads`.
- Unsafe blocks: none.

## Discarded ideas (measured)

| it | idea | result | why |
|---|---|---|---|
| 4 | H3 heap merge engine for pieces > 64 B | composite 0.766 but `whitespace_heavy` 1.096 [1.090, 1.104] | heap push per live pair loses to the rescan below ~192 B; crossover measured per class in `bpe::crossover` (ignored test); re-run at 192 B (it. 5) and 40 B for non-ASCII leads (it. 17), both kept |
| 6 | H7 inline `u64` keys for 1..=7-byte tokens in `FxHashMap<u64>` | 1.458 | FxHash of a `u64` is one multiply; hashbrown indexes by low bits, so tokens sharing a 2-byte prefix chained |
| 7 | H7b same layout with a splitmix64 hasher | 1.016 [1.014, 1.018]; cold start 1.6x | hashing cost up, second 65k-entry table at init |
| 8 | H7c hand-rolled linear-probing table, inline keys, tag bytes | 1.128 [1.124, 1.133]; branch-miss 1.65 -> 2.87% | most merge-loop lookups are misses; hashbrown answers a miss with one 16-byte group compare, linear probing walks tags with a mispredicting exit. The winning variant (it. 19, 24) keeps hashbrown's probe and only changes the key type |
| 10 | H11 branch-free fixed-width key compare instead of `memcmp` | 1.043 [1.041, 1.046] | helped prose 4%, but the length-band branch mispredicts on CJK/whitespace where key lengths vary per probe; needed one `unsafe` block |
| 11 | H12 carry token ranks through the merge (no id-pass lookups) | 0.994 [0.988, 1.000]; `whitespace_heavy` 1.089 | the extra field per part slows `Vec::remove` and the rescan more than the saved lookups; two reworks (8-byte packed part; branch-free u64 min) were worse |
| 15, 16, 26 | merge_scan min-loop variants: 4-lane u64 min; SoA rank stream; two-pass min + position | prescreen cycles 1.00-1.09 | the fused cmov loop is already ~1 cycle/element; anything with a second pass or a second `Vec::remove` loses |
| 18 | H4 count-only path (`Sink` trait, then const-generic `COUNT_ONLY`) | prescreen: `_count` arms 1.04-1.14, encode arms +9-16% | skipping the final per-token lookups saves little (most pieces are whole-token hits), and both generic shapes pessimized the shared merge loop's codegen |
| 20 | H15 12-byte short-table entry | prescreen 0.98-1.01 | entry size is not the limiter |
| 22 | H10 fat LTO + codegen-units=1 (bench-only override) | prescreen 0.97-1.03 mixed | no case for a workspace profile change; recommendation only |
| 23 | H9b SSE2 16-byte class scan ahead of the SWAR word | prescreen 1.01-1.06 | pieces average 2.5-7.5 bytes; a 16-byte probe rarely completes and its setup costs more than the SWAR word it replaces |
| 28, 29 | H18 thread-local `Scratch` reused across calls (RefCell; Cell take/set) | 1.008 [1.003, 1.012]; `whitespace_heavy` 1.058 | `short_strings` gains 6% but long-piece arms lose 2-10%; instructions flat, so this is code layout of `merge_scan`, not the TLS access |
| 30 | H8 direct-mapped piece -> ids cache (4096 x 32 B) | prescreen: code 0.97, mixed 0.82, `short_strings` 5.6x | per-call 128 KiB zeroing; a cross-call cache needs the TLS scratch above; and the synthetic corpus's ~600-word vocabulary inflates the hit rate (overfit risk) |
| 31 | H19 split `class_at` astral path | prescreen +-2% | no signal |
| 33 | H3d heap thresholds 128/24 B after the cheaper heap | 1.004 [1.003, 1.006]; `short_strings` 1.061 | the crossover probe uses homogeneous pieces; real short strings merge less and the heap's fixed setup dominates at 24-40 B |
| 35 | H4b count-only path as a second non-generic loop | prescreen: both encode and count arms 1.09-1.12 | a second call site of `encode_piece` changes inlining for both; the count-only idea is closed after three shapes (18, 35) |
| 34 | H21 bucket queue by rank replacing `BinaryHeap` | prescreen: cjk 1.07, `short_strings` 3.9x, adversarial 29x | 65536 buckets per scratch; leftmost-tie needs a per-bucket min that degenerates when 64k pairs share one rank |

## Open hypotheses

Ranked by expected value; details in `.auto/ideas.md`. None is expected to exceed ~5%
composite without reducing the merge loop's lookup count, which the re-attribution at
iteration 32 (`ATTRIBUTION.md`) identifies as the remaining cost on every multi-token arm.

1. Pre-size `parts` from the longest piece in a first pass to remove the per-call growth
   memmove on short strings without the TLS codegen effect (3-5% on `short_strings` only).
2. Heap engine: skip recomputing the left neighbour's pair rank when a proof shows it cannot
   be selected before the recomputed right pair (needs a differential test).
3. AVX2 runtime-dispatched class scan only inside the heap engine's long-run path.
4. Real-text validation before any piece cache.

## Recommendations needing a human decision

- Workspace release profile: fat LTO + `codegen-units = 1` measured -3%..+3% per arm on this
  crate alone (iteration 22); no change proposed from this branch.
- PGO: not attempted (no representative production corpus on the host; the synthetic corpus
  would overfit). If a production sample exists, `cargo pgo` on `arms --quick` is the recipe.
- ISA baseline: the kept code uses only SWAR and hashbrown's SSE2 group probe (x86-64
  baseline); no `target-cpu` or runtime dispatch was needed, so no deployment-baseline decision
  is required.
- MSRV: `slice::as_chunks` (stable 1.88) is used in `bpe::from_blob`; the workspace pins 1.98.
- `hashbrown` becomes a direct dependency (default features off, no new transitive crates).
- `MAX_PIECE_BYTES`: with the heap engine a 4 KiB chunk is linear-time (97 ns/byte); the cap
  could be raised or removed for exact parity on longer runs, but that changes ids for texts
  the reference chunked and is a product decision, not a perf one.
