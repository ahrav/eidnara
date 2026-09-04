# Tokenizer benchmark design

Written before any bench code. Every later harness change amends this file in its own
iteration, followed by a fresh A/A control.

## Claim under test

"Candidate commit C encodes the fixed workload corpus faster per input byte than the last kept
commit K, on this host, in steady state, with token ids unchanged." Ids are not part of the
metric; `guard.sh` owns correctness and a guard failure is always `discard`.

## Outcome and timing boundary

- Outcome: wall-clock nanoseconds per input byte for one call of the public API on one arm
  fixture, measured inside one process with `std::time::Instant` around the call only. The
  fixture is already in memory as `&str`; no I/O, no allocation of the input, and no result
  inspection is inside the boundary. Dropping the returned `Vec<Rank>` is inside the boundary
  for `encode_ordinary` arms because callers pay it.
- Warm state: the vocabulary `OnceLock` is forced before timing starts, then each arm runs
  `WARMUP` untimed calls, then `ITERS` timed calls. The per-process arm value is the median of
  the `ITERS` timings divided by fixture bytes (`short_strings` divides by the summed bytes of
  all 10k strings; one "call" is the loop over all of them).
- Cold state: `cold_start` is a separate arm and a separate estimand: nanoseconds from process
  start of the timed region to the return of the first `estimate_tokens("hello")` in a fresh
  process (`arms --cold`). It measures vocabulary construction plus one tiny encode. It is
  reported next to the composite and never enters the geomean.

## Workload

Fixed, committed fixtures under `benches/corpus/`, produced once by
`gen/gen-bench-corpus.ts` with a seeded xorshift PRNG (seed in the script). Arms and what
each isolates:

| arm | size | isolates |
|---|---|---|
| `ascii_prose` | ~256 KiB | letter-run pieces, contractions, `' ?\p{L}+'` path |
| `code` | ~256 KiB | punctuation runs, digits, indentation, short pieces |
| `cjk` | ~256 KiB | multi-byte letters, long `\p{L}+` pieces near the merge cap |
| `mixed_unicode` | ~256 KiB | class boundaries on non-ASCII, ECMAScript whitespace set |
| `whitespace_heavy` | ~256 KiB | `\s+(?!\S)` lookahead and long whitespace tokens |
| `numeric` | ~256 KiB | `' ?\p{N}+'` pieces, `[^\s\p{L}\p{N}]+` separators |
| `short_strings` | 10k × 1..64 B | per-call overhead |
| `adversarial_long_piece` | 64 KiB | chunking above `MAX_PIECE_BYTES` |
| `cold_start` | fresh process | vocabulary construction |

Count-only variants (`estimate_tokens` instead of `encode_ordinary`) run on `ascii_prose` and
`code` as their own arms (`ascii_prose_count`, `code_count`) so a count-only fast path is
measurable. All `encode_ordinary` arms and the two count arms enter the composite;
`cold_start` does not.

The corpus is synthetic. It is meant to exercise every pattern alternative and byte class,
not to estimate production token throughput; ratios between commits are the claim, not
absolute ns/byte.

## Observation hierarchy and units

```
timed call  within  process  within  build  within  host session
```

- Treatment application unit: one process launch of one binary.
- Analysis unit: one block of four process launches, template KCCK on even blocks and CKKC on
  odd blocks (restricted counterbalancing; a fixed position effect cancels across the pair of
  templates), giving one paired per-arm log-ratio per block after averaging the two
  same-treatment launches.
- Replication: `REPLICATES >= 5` blocks (default 6; `REPLICATES` env overrides).
- Interference unit: one physical core. Every launch is `taskset -c $CORE` on the same core
  (default core 8, `BENCH_CORE` env). This host reports one thread per core, so SMT is not a
  concern; if the host changes, pick a core whose sibling is idle.
- Build unit: one `cargo build --release` per commit. Build nondeterminism is not part of the
  claim; the baseline binary is the exact artifact kept under `target/keep/arms`.

## Estimator and decision rule (`report.sh`)

Per arm: ratio = candidate / baseline of the per-launch median ns/byte, formed within each
block, then the point estimate is the geometric mean of block ratios and the 95% CI is a
percentile bootstrap (10 000 resamples) over blocks of that geometric mean. Composite =
geometric mean over arms of the candidate arm medians (over launches), printed as the last
stdout line as a bare number. Composite ratio = geomean over arms of the per-arm point
estimates; its CI is the bootstrap over blocks of that quantity.

Verdict printed before the bare number:

- `KEEP` if composite ratio <= 0.99 and composite CI upper bound < 1.0 and no arm has CI lower
  bound > 1.03.
- `keep (simpler)` is a human/agent annotation, not computed: equal performance (composite CI
  contains 1.0 and no arm regresses per the rule above) with less code or fewer dependencies.
  `report.sh` prints `NEUTRAL` in that case so the caller can apply it.
- `DISCARD` otherwise.

A/A control: run `report.sh --aa` (candidate = baseline binary). Harness is acceptable when
the composite ratio is within 1% of 1.0 and every arm ratio is within 3% of 1.0 (the same
bounds the decision rule uses). Re-run after any harness change and every 25 iterations. The
A/A run is a calibration check of the whole path, not a noise floor: a KEEP still needs its own
CI.

## Secondary signals (never decide KEEP alone)

- Instruction counts: `arms --quick` under `valgrind --tool=cachegrind` per arm, total
  `Ir`. Used to triage sub-1% ideas.
- `perf stat -e cycles,instructions,branches,branch-misses,L1-dcache-load-misses,cache-references,cache-misses,l1_dtlb_misses`
  around one `arms --quick` launch of the candidate; IPC and miss rates go into `results.tsv`.
  On this VM `L1-dcache-load-misses` reads 0 (event not exposed by the hypervisor); the
  `cache-*` and `l1_dtlb_misses` events do count and stand in for it.

## Reporting contract

Each iteration appends one row to `results.tsv`: iteration, commit, altitude, hypothesis id,
composite ns/byte, composite ratio and CI, per-arm ratio and CI, cold-start ns, IPC,
instructions, verdict, reason. `BASELINE.md` records host, kernel, governor, toolchain, commit,
the Phase 0 absolute numbers, and the A/A result. `REPORT.md` is the running summary.

## Known threats

- Single core, single host: results generalize to this microarchitecture only.
- No frequency governor control is exposed on this VM (`/sys/devices/system/cpu/*/cpufreq`
  absent); drift is handled by ABBA blocking, not by pinning frequency.
- Synthetic corpus: arm mix does not estimate production throughput.
- Block count is small; CIs are percentile bootstraps over 5-6 blocks and are approximate.
