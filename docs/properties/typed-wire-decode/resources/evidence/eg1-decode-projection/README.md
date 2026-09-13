# EG1 decode-projection payoff: before leg

Status: before leg collected; after leg pending (the typed-wire U1 ticket).
Gate definition: [`../../evidence-gates.md`](../../evidence-gates.md).

The `before/` directory is the frozen before leg of the
`typed-wire-resources-measurement-pair` receipt:

- `manifest.json`: source and binary identities, toolchain, build, host, corpus
  checksums and distributions, exact operation boundaries, replication,
  per-process estimates, and the predeclared gain/noise rule.
- `harness.patch`: the only change applied over production revision `b73ca464`
  to obtain the bench binary (the `decode` group and the corpus dump in
  `crates/daemon/benches/hot_path.rs`). Its SHA-256 is in the manifest.
- `corpus/`: the exact request bodies each cell decodes, with SHA-256 in the
  manifest.
- `raw/<cell>/<size>/r<N>/`: Criterion `sample.json`, `estimates.json`, and
  `benchmark.json` for each of five independent processes.

Collection command, run once per process from a detached worktree at
`b73ca464` with `harness.patch` applied:

```sh
cargo +1.98 bench -p daemon --locked --features bench-internals --bench hot_path --no-run
EIDNARA_DUMP_DECODE_CORPUS=<corpus-dir> CRITERION_HOME=<criterion-dir> \
  target/release/deps/hot_path-<hash> --bench decode --save-baseline before-r<N> --noplot
```

The after leg must reuse this patch on its final tree, alternate before/after
process order AB, BA, AB, BA, AB across five pairs, and apply the manifest's
predeclared rule per cell. Nothing here is a payoff verdict.

Before-leg process-mean medians and relative spreads (max minus min over the
median of the five process means), which fix each cell's noise floor:

| Cell | Median of process means (ns) | Relative spread |
| --- | --- | --- |
| `decode/typed_request/40msgs_2KiB_mixed` | 223291 | 0.0053 |
| `decode/typed_request/200msgs_2KiB_mixed` | 1280939 | 0.0378 |
| `decode/typed_request_plus_projection/40msgs_2KiB_mixed` | 687275 | 0.0212 |
| `decode/typed_request_plus_projection/200msgs_2KiB_mixed` | 3326626 | 0.0232 |
