# Phase 0 baseline

- Commit: d35e2b7 (`u3/5-tokenizer` tip, PR #22) + harness (iteration 0)
- Host: AMD EPYC 9R14 (Zen 4), 2 sockets x 64 cores, 1 thread/core, 2 NUMA nodes; bench core 8
- Kernel: 6.12.100-125.179.amzn2023.x86_64; cpufreq governor: not exposed (`/sys/devices/system/cpu/*/cpufreq` absent); THP: madvise
- Toolchain: rustc 1.98.1 (48a229cea 2026-09-01) via rust-toolchain.toml (channel 1.98); `cargo build --release` default profile
- Implementation: `tiktoken-rs 0.11.0` CoreBPE + `fancy-regex 0.17` pre-tokenizer, `FxHashMap<Vec<u8>, Rank>`

## Absolute numbers (kept binary, median over 12 launches x 25 timed calls)

| arm | ns/byte |
|---|---|
| adversarial_long_piece | 1356.51 |
| ascii_prose | 79.27 |
| ascii_prose_count | 79.43 |
| cjk | 56.76 |
| code | 152.63 |
| code_count | 152.95 |
| mixed_unicode | 97.26 |
| numeric | 179.43 |
| short_strings | 81.29 |
| whitespace_heavy | 112.65 |
| **composite (geomean)** | **133.58** |
| cold_start (ns, fresh process, first estimate_tokens("hello")) | 27555088 |

perf stat over one `arms --quick` launch: IPC 3.48, 9.1 G instructions, branch-miss 0.62%.
Size: libtokenizer.rlib 3,416,376 B; arms bench binary 6,523,464 B (`benches/baseline.json`).

## A/A control (same binary as K and C, 6 blocks KCCK/CKKC)

composite ratio 0.9994 [0.9909, 1.0085]; every arm ratio within 1% of 1.0; verdict `AA_OK`.
An earlier 5-block KCCK-only run showed composite 1.0055 [1.0007, 1.0143] with a systematic
position effect (second launch slower); the alternating KCCK/CKKC template fixed it, and that
change is what DESIGN.md now specifies.
