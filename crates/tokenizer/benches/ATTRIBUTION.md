# Attribution (Phase 1, iteration 1, no source change)

Binary: iteration-0 kept build with `CARGO_PROFILE_RELEASE_DEBUG=1` (symbols only; codegen
otherwise identical). Tools: `perf record -F 4999 -g` per arm (`target/prof/*.svg`
flamegraphs, `*.folded`), one `--call-graph dwarf` run on `ascii_prose` for inlined callers,
`valgrind --tool=cachegrind --cache-sim=no` and `--tool=dhat` on `arms --quick`.

## Where time goes (self time, % of arm samples)

| arm | fancy-regex VM (`vm::run`, `State::save`, `Matches::next`) | regex-automata DFA/lazy-DFA | `memcmp` (from `vm::matches_literal`) | malloc/free/grow | tiktoken merge | other |
|---|---|---|---|---|---|---|
| ascii_prose | 42 | 18 | 13 | 13 | 2 | 12 |
| code | 40 | 27 | 11 | 12 | 2 | 8 |
| numeric | 36 | 24 | 13 | 10 | 5 | 12 |
| whitespace_heavy | 36 | 29 | 9 | 10 | 10 | 6 |
| mixed_unicode | 33 | 26 | 10 | 9 | 8 | 14 |
| short_strings | 33 | 19 | 10 | 9 | 13 | 16 |
| cjk | 20 | 16 | 10 | 5 | 37 | 12 |
| adversarial_long_piece | 0 | 0 | 0 | 0 | 82 | 15 (memmove from `Vec::remove`) |

Stack-inclusive: on `ascii_prose` 63% of samples are under `fancy_regex`/`regex_automata`
frames, 2.4% under `_byte_pair_merge`, and 13% of leaf samples are the allocator.

## Mechanism

- **(a) Pre-tokenizer regex dominates every arm but `cjk` and the adversarial one.**
  `fancy-regex` cannot delegate the whole pattern to `regex-automata` because of the
  `(?!\S)` lookahead, so it runs its backtracking VM over every alternative. Per piece it:
  runs the lazy DFA for the delegated sub-expressions (`hybrid::search::find_fwd`,
  `meta::Core::search_half`), compares literal alternatives (`'s|'t|'re|...`) via
  `matches_literal` -> `memcmp` (13% of `ascii_prose` on 1-3 byte compares), and pushes/pops
  `Save`/`Branch` state on heap `Vec`s (`RawVec<Save>::grow_one`, 7.6% inclusive). It also
  allocates a `Vec<usize>` of capture positions per match and frees it (4.7%). dhat: 77% of all
  heap blocks (1.14 M of 1.49 M in the quick run) come from `fancy_regex`; there are 54 k pieces
  in `ascii_prose` and ~4 allocations per piece.
- **(b) Rank lookup hashing is not visible as a separate cost.** `FxHashMap<Vec<u8>, Rank>`
  lookups take `&[u8]` (no allocation); their hashing shows up inside `_byte_pair_merge` and is
  small on all arms except `cjk`.
- **(c) Merge loop.** `_byte_pair_merge` is O(n·m) with a `Vec::remove` per merge; costs 37% on
  `cjk` (pieces average 20 bytes, up to 4 KiB) and 82% on the 64 KiB adversarial piece
  (`memmove` from `remove` is the other 15%). For ASCII arms the mean piece is 2.5-4.8 bytes and
  most pieces hit the whole-piece `encoder.get(piece)` shortcut, so the merge loop is ~2%.
- **(d) Allocation.** 20% of blocks are tiktoken's (`parts` Vec per merged piece plus the
  per-piece `Vec<Rank>` from `byte_pair_encode` that is then `extend`ed into `ret`), 77% are
  fancy-regex's. Total 1.49 M blocks / 103 MB for one pass of `ascii_prose` + `short_strings`.
- **(e) Output Vec growth.** `ret = vec![]` grows by doubling; `finish_grow` is 4.8% on
  `ascii_prose`, shared between output growth and fancy-regex's stacks.
- **(f) OnceLock init.** Not on the warm path. Cold start is 27.6 ms: base64 decode of 65 k
  lines + `FxHashMap` build + `CoreBPE::new`, which also builds a sorted `Vec<Vec<u8>>` for
  `decoder` and compiles the pattern twice (one per thread-local slot). `cachegrind` shows
  `quicksort::<Vec<u8>>` at 3% of the quick run: that is `CoreBPE::new` sorting the vocabulary.

Instruction budget (`cachegrind`, `arms --quick`, 12.1 G Ir): `memcpy` 22.9% (almost all from
the adversarial `Vec::remove` shifts), `vm::run` 12.8%+2.7%+1.6%, `_byte_pair_merge`
9.6%+4.6%+2.4%+2.3%, `_int_free`+`malloc`+`free` 7.4%, `memcmp` 3.3%, regex-automata ~6%.

## Ranked hypotheses

| id | altitude | hypothesis | expected effect | evidence |
|---|---|---|---|---|
| H1 | A | Replace `fancy-regex` pre-tokenizer with a hand-written scanner (ASCII table + Unicode L/N range tables pinned to the regex crate's Unicode version; whitespace lookahead as "give back last whitespace char if a non-whitespace char follows"). | -50..-65% on ascii_prose/code/numeric/whitespace/mixed/short; removes ~4 allocs per piece | (a), (d) |
| H2 | A | In-crate BPE with tiktoken's exact merge order; whole-piece lookup first; 2-byte direct table for the initial pair ranks; reusable scratch (no `parts` alloc, no per-piece `Vec<Rank>`). | -20..-35% on cjk, -5% elsewhere once H1 lands | (c), (d) |
| H3 | A | Linear-time merge for long pieces (heap or linked-list variant, equivalence argued + differential) so `adversarial_long_piece` stops paying `Vec::remove` memmoves. | -80% on adversarial arm only (1 of 10 arms, ~15% composite) | (c), cachegrind memcpy 23% |
| H4 | A | Count-only path for `estimate_tokens`: count = pieces + merges avoided, no output Vec. | -5..-10% on the two `_count` arms | (e) |
| H5 | A | Cold start: precomputed vocabulary layout (`build.rs` or `include_bytes!` with checksum) instead of parsing base64 at first call; skip `CoreBPE::new`'s decoder sort. | cold 27.6 ms -> < 3 ms; not in composite | (f) |
| H6 | B | Output `Vec::with_capacity(text.len() / 4)` or count-then-fill. | -1..-3% on encode arms | (e) |
| H7 | B | Rank lookup: borrowed-key hash on `&[u8]` with a length-keyed mix, or perfect hash / sorted 1-3 byte tables. Only measurable after H1/H2 expose it. | unknown until re-attribution | (b) |
| H8 | A | Piece -> ids cache for repeated words. | uncertain; discard unless short_strings/code gain | (a) |
| H9 | C | SWAR/SIMD ASCII class-boundary scan inside the H1 scanner, runtime-dispatched. | -10..-20% on ASCII arms after H1 | (a) |
| H10 | D | LTO/codegen-units=1/panic=abort report; PGO report. | -3..-8% typical | recommendation only |

Order of attack: H1, then re-attribute (profile moves), then H2/H3/H4, H5 as its own iteration,
then B/C altitudes.

## Re-attribution at iteration 32 (current best, 8.73 ns/byte composite)

`perf record` per arm on the kept binary with debug symbols (`target/prof/*.data`):

| arm | ns/B | top self-time symbols |
|---|---|---|
| ascii_prose | 2.56 | `encode_piece` 54% (mostly the inlined whole-piece hash probe), `encode_bounded` 21% (scanner, inlined), `merge_scan` 14% |
| code | 3.56 | `encode_piece` 43%, `encode_bounded` 24%, `merge_scan` 21% |
| cjk | 16.8 | `merge_heap` 60% (heap push/pop 15%, rank lookups the rest), `merge_scan` 25% |
| whitespace_heavy | 13.8 | `merge_scan` 71% (rescan min loop 40%, span lookups 30%) |
| numeric | 12.1 | `merge_scan` 59%, `encode_piece` 18%, `encode_bounded` 16% |
| short_strings | 15.8 | `merge_scan` 45%, `encode_piece` 18%, malloc/free/memmove 7% (per-call Vec growth) |
| mixed_unicode | 11.6 | `merge_scan` 55%, `class_at` 12%, `class_from_tables` (astral chars) 5% |

What moved since Phase 1: the pre-tokenizer went from 55-70% to ~20% of ASCII arms; the
allocator from 10-13% to <1% on long texts; the merge loop is now the dominant cost on every
arm that has multi-token pieces (36-70%). Inside it, the O(n·m) rescan's min loop is at ~1
cycle/element and three rewrites (iterations 15, 16, 26) did not beat it; the remaining cost
is rank lookups for candidate spans, which the inline-key tables (iterations 19, 24) already cut.

Remaining hypotheses ranked by expected value are in `REPORT.md` and `.auto/ideas.md`; none
is expected to exceed ~5% composite without changing the merge algorithm's lookup count.
