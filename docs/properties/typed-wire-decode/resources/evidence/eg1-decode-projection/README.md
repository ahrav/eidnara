# EG1 decode-projection payoff: before and after legs

Status: both legs collected; verdict `proceed` on all four cells.
Gate definition: [`../../evidence-gates.md`](../../evidence-gates.md).

## Before leg

The `before/` directory is the frozen before leg of the
`typed-wire-resources-measurement-pair` receipt:

- `manifest.json`: source and binary identities, toolchain, build, host, corpus
  checksums and distributions, exact operation boundaries, replication,
  per-process estimates, and the predeclared gain/noise rule.
- `harness.patch`: the only change applied over production revision `85accd89`
  to obtain the bench binary (the `decode` group and the corpus dump in
  `crates/daemon/benches/hot_path.rs`). Its SHA-256 is in the manifest.
- `corpus/`: the exact request bodies each cell decodes, with SHA-256 in the
  manifest. The before-leg binary built these bodies through the serializer
  of revision `85accd89` and dumped the same `body` value it timed. The
  committed `bench_decode` now `include_bytes!` these two files instead, so a
  later revision whose serializer omits fields (for example
  `provider_executed: false` after canonical block bytes landed) still decodes
  the same bytes. Rebuilding the corpus on such a revision yields bodies 520
  and 2,600 bytes smaller; do not regenerate it.
- `raw/<cell>/<size>/r<N>/`: Criterion `sample.json`, `estimates.json`, and
  `benchmark.json` for each of five independent processes.

Collection command, run once per process from a detached worktree at
`85accd89` with `harness.patch` applied:

```sh
cargo +1.98 bench -p daemon --locked --features bench-internals --bench hot_path --no-run
EIDNARA_DUMP_DECODE_CORPUS=<corpus-dir> CRITERION_HOME=<criterion-dir> \
  target/release/deps/hot_path-<hash> --bench decode --save-baseline before-r<N> --noplot
```

The after leg runs the committed `decode` group on its final tree (not
`harness.patch`, whose corpus rebuild is serializer-dependent), confirms the
bytes each cell decoded equal the manifest corpus, alternates before/after
process order AB, BA, AB, BA, AB across five pairs, and applies the manifest's
predeclared rule per cell.

Before-leg process-mean medians and relative spreads (max minus min over the
median of the five process means), which fix each cell's noise floor:

| Cell | Median of process means (ns) | Relative spread |
| --- | --- | --- |
| `decode/typed_request/40msgs_2KiB_mixed` | 223291 | 0.0053 |
| `decode/typed_request/200msgs_2KiB_mixed` | 1280939 | 0.0378 |
| `decode/typed_request_plus_projection/40msgs_2KiB_mixed` | 687275 | 0.0212 |
| `decode/typed_request_plus_projection/200msgs_2KiB_mixed` | 3326626 | 0.0232 |

## After leg

The `after/` directory holds the paired comparison for the typed-wire U1 tree
(commit `3010d73d` on `perf/typed-wire-u1-owned-decode`):

- `manifest.json`: after-tree and before-tree identities, both binary digests,
  the frozen corpus digests and each binary's own dump of the bytes it decoded,
  the same operation boundaries and predeclared rule, per-process estimates for
  both legs, and the per-cell verdict.
- `raw/before/<cell>/<size>/r<N>/` and `raw/after/<cell>/<size>/r<N>/`:
  Criterion `sample.json`, `estimates.json`, and `benchmark.json` for each of
  the five paired processes per leg.

The before binary was rebuilt from `85accd89` plus `harness.patch` in a
detached worktree. Its SHA-256 equals the before-leg manifest's binary digest,
so the reproduction is the same artifact. The after binary was built from the
U1 tree with the same command. The after tree's `bench_decode` embeds the
frozen `before/corpus/` bodies with `include_bytes!`, so both legs decoded the
same bytes; the timing boundaries, iteration schedule, and drop placement are
those of `harness.patch`. Both binaries dumped the bytes they decoded, and the
digests match the frozen corpus.

Ten fresh processes ran in the order AB, BA, AB, BA, AB (A before, B after),
each with its own `CRITERION_HOME`:

```sh
EIDNARA_DUMP_DECODE_CORPUS=<dump-dir> CRITERION_HOME=<criterion-dir> \
  <binary> --bench decode --save-baseline <leg>-r<N> --noplot
```

Two earlier after-leg runs were discarded before analysis. In the first the
after binary generated its own bodies, which lack the twenty
`provider_executed:false` members the frozen bodies carry (520 fewer bytes at
40 messages). The second read the frozen bodies through an environment
variable; the bench was then changed to embed them so the artifact does not
depend on the environment, and only the run against that binary is reported.

Per cell, medians of the five process means (ns), the after leg's relative
spread, the median per-pair after/before ratio, the predeclared proceed
threshold (1 minus twice the before-leg noise floor), and the number of pairs
whose after mean is below the before mean by more than twice the noise floor:

| Cell | Before rerun median | After median | After spread | Median ratio | Threshold | Pairs clearing | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `decode/typed_request/40msgs_2KiB_mixed` | 223692 | 123256 | 0.0179 | 0.5506 | 0.9895 | 5/5 | proceed |
| `decode/typed_request/200msgs_2KiB_mixed` | 1313136 | 607338 | 0.0200 | 0.4656 | 0.9245 | 5/5 | proceed |
| `decode/typed_request_plus_projection/40msgs_2KiB_mixed` | 701702 | 517366 | 0.0095 | 0.7364 | 0.9576 | 5/5 | proceed |
| `decode/typed_request_plus_projection/200msgs_2KiB_mixed` | 3346128 | 2509362 | 0.0026 | 0.7484 | 0.9536 | 5/5 | proceed |

The before rerun medians sit within each cell's original noise floor of the
frozen before-leg medians. The predeclared rule clears on every cell, so the
whole-plan stop for a gain within noise does not fire. This is the narrow
decode-plus-projection operation on one host; it is not a production latency
claim.
