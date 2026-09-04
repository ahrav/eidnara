# Autoresearch: optimize the `tokenizer` crate (PR #22)

Full runbook: `docs/runbooks/autoresearch-tokenizer-perf.md` in the main checkout
(`../eidnara/docs/runbooks/autoresearch-tokenizer-perf.md`); a copy of its invariants and
ladder is condensed here. Read `crates/tokenizer/benches/DESIGN.md`, `ATTRIBUTION.md`,
`results.tsv`, and `REPORT.md` before every iteration.

## Objective

Make `tokenizer::estimate_tokens` and `tokenizer::encode_ordinary` as fast as possible on the
fixed corpus under `crates/tokenizer/benches/corpus/`, with token ids bit-identical to the
reference implementation (`src/reference_impl.rs`, the tiktoken-rs + fancy-regex path). Lower
is better. Iterations: unlimited. Plateau patience: off. Never ask whether to continue. When
stuck, change altitude (design -> data structure -> micro -> build, or back up).

## Metrics

- **Primary**: `composite_ns_per_byte` (ns/byte, lower is better): geomean over 10 arms of
  the candidate's median ns/byte (arms: ascii_prose, ascii_prose_count, code, code_count, cjk,
  mixed_unicode, whitespace_heavy, numeric, short_strings, adversarial_long_piece).
- **Decision statistic** (not the primary number): `composite_ratio` with bootstrap CI, per-arm
  ratios. `report.sh` prints `VERDICT KEEP|NEUTRAL|DISCARD`. Apply it verbatim: KEEP only if
  composite ratio <= 0.99 AND CI upper < 1.0 AND no arm CI lower > 1.03. NEUTRAL with less code
  or fewer deps = `keep (simpler)`. Anything else = discard.
- **Secondary**: `cold_start_us` (fresh-process first call; tracked separately; guard fails if
  > +20% over `benches/baseline.json` unless `GUARD_COLD_TRADE=1` and logged), `ipc`,
  `instructions_g`, `branch_miss_pct`.

## How to run

- `./.auto/measure.sh` -> runs `crates/tokenizer/benches/report.sh` (6 ABBA blocks, core 8,
  ~3 min), prints the per-arm table, `VERDICT`, and `METRIC` lines.
- `./.auto/checks.sh` -> runs `crates/tokenizer/benches/guard.sh` (~15 s). Runs automatically
  after measure when using the autoresearch tools; a failure blocks keep.
- After a keep: `crates/tokenizer/benches/report.sh --promote` so `target/keep/arms` is the
  new baseline binary. Then append to `crates/tokenizer/benches/results.tsv` and update
  `REPORT.md`.
- `crates/tokenizer/benches/report.sh --aa` -> A/A control (run after any harness change and
  every 25 iterations; must print `AA_OK`).
- Cachegrind triage for sub-1% ideas: `valgrind --tool=cachegrind <arms binary> --quick`.

## Files in scope

`crates/tokenizer/**`: `src/` (except the two files below), `benches/`, `Cargo.toml`, a new
`build.rs`, additions under `testdata/`, scripts under `gen/`. Root `Cargo.lock` only as a
consequence of `crates/tokenizer/Cargo.toml`.

## Off limits

- `crates/tokenizer/src/reference_impl.rs` (frozen oracle), `benches/guard.sh`,
  `gen/gen-diff-corpus.ts`, `benches/corpus/*`, existing entries in
  `testdata/token-golden.json`, assertions in `tests/token_golden.rs`. Harness changes
  (`report.sh`, `stats.py`, `arms.rs`, `measure.sh`) are their own iterations followed by an
  A/A control and a baseline re-pin; never to make a failing experiment pass.
- No push, no PR edits. Leave the branch and `REPORT.md` for a human.

## Hard invariants (guard enforces; violation = discard)

1. Public API: `estimate_tokens(&str) -> usize`, `encode_ordinary(&str) -> Vec<Rank>`,
   `MAX_PIECE_BYTES: usize`, `pub use Rank` (u32). Additive API fine.
2. Ids bit-identical to reference for every piece <= MAX_PIECE_BYTES;
   `estimate_tokens(t) == encode_ordinary(t).len()` always.
3. Pre-tokenizer boundaries exactly `CLAUDE_PAT_STR` (ECMAScript whitespace class: U+FEFF in,
   U+0085 out; `(?![^ws])` trailing-whitespace rule).
4. Merge order is tiktoken's: lowest rank first, ties to the leftmost pair.
5. Linear worst case above MAX_PIECE_BYTES (chunking contract or strictly better).
6. No `unsafe` unless the change is a keep on its own and carries a `// SAFETY:` proof; load
   /low-level-systems:safe-over-unsafe first.
7. `cargo fmt --check`, `clippy --all-targets -D warnings`, `cargo test`, `cargo doc` with
   `-D warnings`, `build --no-default-features` all pass.
8. Never rely on `target-cpu=native`; ISA above x86-64-v2 only behind runtime dispatch with a
   scalar fallback.

## Iteration protocol

1. Read `git log`, `results.tsv`, `ATTRIBUTION.md`; pick the highest-expected-value untested
   hypothesis at the current altitude (three consecutive discards -> change altitude).
2. One change. Commit as `experiment: <altitude> <hypothesis id> <one line>`.
3. `checks.sh` (guard). Fail -> up to 2 reworks, then revert, log `discard (guard)`.
4. `measure.sh`. Apply the verdict verbatim. Discard -> revert, log with CI.
5. Append a row to `results.tsv` (iteration, commit, altitude, hypothesis, composite, per-arm
   ratio+CI, cold ns, IPC, instructions, verdict, reason) and `.auto/log.jsonl`.
6. Every 10 iterations re-run Phase 1 attribution on the current best; every 25, A/A control.

## Exploration ladder (details in the runbook)

- **A design**: hand-written scanner replacing fancy-regex (Unicode L/N tables pinned to the
  regex crate's version, drift test); in-crate BPE (min-scan / heap hybrid, tiktoken order);
  rank lookup (borrowed-key hash, perfect hash, direct 2-byte table, trie/FST); count-only
  path for `estimate_tokens`; piece cache (discard unless short_strings/code benefit); compile-
  time vocabulary tables for cold start; MAX_PIECE_BYTES only if complexity class changes.
- **B data layout**: SoA merge state, reusable scratch, output capacity estimates, hot-token
  compact tables, no UTF-8 re-validation.
- **C micro**: SWAR/SIMD ASCII class scan behind runtime dispatch, branchless min-pair scan,
  hash function choice, inline/#[cold] placement; check codegen with `cargo asm`.
- **D build**: LTO/codegen-units/panic=abort only as recommendations unless workspace policy
  allows; PGO report only; verify dispatch overhead on short_strings.

## What's been tried

Full table with CIs: `crates/tokenizer/benches/results.tsv`; narrative and discard reasons:
`crates/tokenizer/benches/REPORT.md`; profiles: `ATTRIBUTION.md`. Summary after 34 iterations
(133.58 -> 8.73 ns/byte composite, 15.3x; cold start 27.6 -> 3.2 ms):

- Kept: hand-written scanner (it. 2), in-crate BPE (3), heap engine above 192 B / 40 B
  non-ASCII (5, 17), SWAR 8-byte class scan (9), two-bit BMP class table (12), build.rs
  vocabulary blob (13), output `with_capacity(len/3)` (14), hashbrown `HashTable` with inline
  u64 keys for 3..=7 B tokens (19) and u128 keys for 8..=15 B (24), `#[inline(always)]
  rank_of` (21), ASCII-lead fast path in `piece_end` (27), packed u64 heap key (32).
- Discarded, do not retry as-is: FxHash/splitmix/hand-rolled inline-key tables (6, 7, 8: the
  hashbrown probe is the part that matters), inline `memcmp` replacement (10), carrying token
  ranks through the merge (11), three rescan min-loop rewrites (15, 16, 26), count-only path
  via generics (18), 12-byte short entries (20), LTO (22, recommendation only), SSE2 16-byte
  scan (23), thread-local scratch (28, 29), piece cache (30), astral class_at split (31),
  lower heap thresholds (33), bucket queue (34).
- Harness: A/A at iterations 0 and 25 both `AA_OK`.
