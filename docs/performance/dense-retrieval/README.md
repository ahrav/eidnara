# Dense retrieval: prune judgments before redesigning storage

## Decision

**Pursue local optimization: single-pass, score-first page admission.** Keep
the exhaustive oracle unchanged. Do not implement fixed-small-shortlist SQL
refill or introduce ANN from this evidence. This PR contains research only.

The opportunity is real, but the original audit overstates its deployment
relevance: at source `8e049122`, host search uses history/notes substring search.
`daemon::vector_reader::rank` and dense layered ranking are callable code with
test callers, not the host's wired retrieval path. These results are library
retrieval measurements, not user-facing search latency measurements.

## Baseline and limiting mechanism

The initial real-kernel fixture took **501 ms** per exhaustive query at 20,000
rows, 19,000 live after tombstones, 384 dimensions, k=20, and pages of 1,024.
Five repeated stock samples ranged from 492 to 509 ms. Mean instrumented phases
were about 350 ms for kernel page judgments, 126 ms for projection SQL/framework,
12 ms for decoding, 9 ms for scoring/selection, and 0.65 ms for final revalidation.
Hooked and unhooked totals were close; hooks charge one final row decode per page
to judgment. They do not measure SQLite allocation or hardware memory bandwidth.

Kernel judgment is not just a second narrow SELECT. It derives served classes
from admission history and related facts, then checks registry, artifacts, and
scopes. It already uses named-ID batching and per-batch memoization. Skipping
noncompetitive judgments removes real work rather than accelerating dot products.

Thread CPU time tracked elapsed time. Repeated queries reported zero storage
`read_bytes`, but hundreds of MB of `rchar` and tens of thousands of read calls.
The observed bottleneck is CPU/SQLite materialization and page-cache access, not
measured disk stalls or single-query contention. `rchar` is bytes returned by
read-family syscalls, not DRAM traffic. No PMU bottleneck claim is made.

Connection configuration matters. The helper uses SQLite defaults; the daemon
pins an 8 MiB projection cache and memory temp storage. Final validation uses
those settings. Do not transfer the helper's absolute numbers to that path.

## Experiments

Times below are repeated-query medians in milliseconds. Each row compares paths
within one process on the same frozen fixture. All successful rankings matched
independently sorted IDs and exact f64 score bits. “Exact” here does **not** mean
the prototypes preserve the oracle's complete diagnostic/error contract.

| Question and workload | Stock | Page pruning | SQL shortlist | Resident shortlist | Decision |
|---|---:|---:|---:|---:|---|
| Final SQL, 20k, all admitted, daemon projection settings, 3 repeats | 685.65 | 348.54 | 313.20 | 9.36 | Keep page pruning; resident remains a redesign candidate |
| Final file-backed `rank_layers`, same fixture, 3 repeats | 781.58 | 435.82 | — | — | Keep: 1.79x measured library speedup |
| Final SQL, 20k, 1% prefix admission, query seed 2, 2 repeats | 443.20 | 383.11 | 8,304.68 | 214.81 | Reject fixed-small-shortlist refill |
| Pilot SQL, 100k, all admitted, helper defaults, 3 repeats | 3,943.78 | 802.75 | 700.42 | 33.02 | Scaling opportunity; not a production estimate |
| Pilot SQL, 2k, k=256, 50% prefix admission, seed 3, 3 repeats | 21.84 | 19.74 | 12.10 | 7.69 | Page benefit is small when k is large relative to N |

Final file-backed ranges were **779.60–785.33 ms** stock and
**435.38–438.12 ms** candidate. Both paths read exactly 19,000 vectors, or
29,184,000 bytes, from the same already-open file. Kernel judgments fell from
19,020 to **1,116**, including final revalidation. File construction and fixture
admission are excluded and recorded separately. This path invokes public
`rank_layers` with an original-row file accessor; it does not measure daemon
acquisition, ledger admission, sidecar verification, or host dispatch.

At 1% admission, both shortlists made **27 complete scans**, scoring 513,000 rows.
They retained exact winners but lost the intended work bound. A fixed oversampling
factor is neither a sufficiency proof nor a request-work bound. Page pruning
kept one scan; it never prunes while the eligible heap is underfilled.

Two barrier-started callers sharing one projection connection produced roughly
0.67/1.35 s stock response pairs and 0.34/0.68 s page-pruning pairs. Connection
serialization remains. These are response times including waits, not an
open-loop capacity test or evidence of parallel scaling.

### Headroom and alternatives

Removing the initial fixture's entire page-judgment phase while holding other
work constant gives an **idealized 3.36x ceiling**. With daemon projection
settings, the measured judgment fraction is smaller; the equivalent ceiling is
about **2.06x** for that SQL path. These are conditional phase-removal models,
not theoretical maxima. Actual file-backed improvement was 1.79x.

The strongest contrary evidence favors resident representation: **9.36 ms/query**
on the final all-admitted fixture. But building that column took **327 ms** and
allocated **52.3 MB** of vector/ID capacity for 29.2 MB of live f32 values.
Two unchanged-snapshot queries amortize that build against the 313 ms SQL
shortlist in this fixture. The prototype rejects any changed projection/kernel
snapshot. It has no incremental liveness diff, durable bitmap, recovery path,
memory admission, or eviction policy. At 1% admission it still spends 215 ms on
repeated scans. A resident single-pass page strategy is unmeasured.

A generous kernel-cache pilot reduced 100k stock latency to about 2.31 s but
also changed projection read behavior. It can allow 256 MiB per kernel reader
and does not establish stable steady state or per-connection attribution.
Do not adopt that budget or the fixture's deprecated persistent-cache pragma.
Configuration isolation confirmed the earlier SQL discrepancy tracks explicit
projection reconfiguration, not simply a larger cache allowance. Its internal
SQLite cause remains unproven and is not needed for the pruning recommendation.

## Recommended implementation boundary

Use the same keyset traversal, vector source, f64 scorer, and tie comparator.
Validate each page, score it, and judge only candidates that can beat the worst
already-eligible kth row. Until k eligible rows exist, admit the whole candidate
page. Preserve final authorization and snapshot/incarnation checks. This needs
O(page × dimension + k) query scratch, not an N-row score array or repeated scans.
File layer resolution retains its separate O(N) scratch; pruning does not remove it.

Do **not** silently replace `dense::oracle::exhaustive`. Its public contract
counts all exclusions and judgments, reports coverage, and refuses malformed
metadata even for losers. The retained guard demonstrates stock refusing a
malformed losing-row digest while the SQL shortlist returns the same winners.
Keep cheap candidate validation before pruning and explicitly agree on a
serving-result contract; leave full-population diagnostics in the oracle.

Before production implementation, the smallest required validation is a bounded
serving-path prototype through the real pinned-vector reader and resource ledger,
with tests for cancellation, total visit limits, classification changes, restore,
missing vectors, malformed losing metadata, and base/delta precedence. Reuse the
oracle and existing mutation hooks. Approval of that contract is required; this
research does not authorize production edits.

## Reproduce and inspect evidence

Use an installed Rust 1.98 toolchain, Python 3, Git, and Linux `taskset` when
pinning. No dependencies are updated. Pick CPUs allowed on the test host.

```sh
git worktree add --detach /tmp/opencode/dense-retrieval-lab \
  8e0491225a7292ef077c675d44b94f94a24041d3

python3 docs/performance/dense-retrieval/run.py \
  --worktree /tmp/opencode/dense-retrieval-lab \
  --output /tmp/opencode/dense-reproduce-20k --cpu 16,17 \
  --candidates --file-layer --concurrent --reps 3 --projection-cache-kib 8192

python3 docs/performance/dense-retrieval/run.py \
  --worktree /tmp/opencode/dense-retrieval-lab \
  --output /tmp/opencode/dense-reproduce-selective --cpu 16 \
  --candidates --selectivity 1 --query 2 --reps 2 --projection-cache-kib 8192

python3 docs/performance/dense-retrieval/run.py \
  --worktree /tmp/opencode/dense-retrieval-lab \
  --output /tmp/opencode/dense-reproduce-guards --guards --cpu 16

python3 docs/performance/dense-retrieval/summarize.py \
  /tmp/opencode/dense-reproduce-20k /tmp/opencode/dense-reproduce-selective
```

The runner copies test-only artifacts into the scratch worktree; no source
module imports this directory. It refuses existing output directories and
records source/binary hashes, commands, compiler versions, CPU identity, setup,
every query sample, resource counters, and correctness outcomes. Temporary
fixture databases are deleted after each process. Raw local attempts are under
`/tmp/opencode/dense-*`; normalized retained samples are in [results/](results/).
Hostnames are redacted in retained manifests; timing/count data is unchanged.

- [Research log](research-log.md): hypotheses, schedules, failures, decisions,
  surprises, and resume state after each iteration.
- [Probe](probe.rs), [candidates](candidates.rs), [file accessor](file_layer.rs),
  [guards](guards.rs): exact source for final-artifact runs.
- `results/dense-final-*.json` and `results/dense-config-*.json`: final-artifact
  evidence. Other result files are pilots; not all pilot source versions remain.
- Existing release-mode dense tests: **76 passed** across `dense_oracle`,
  `dense_layered`, `dense_properties`, `dense_resolve`, and `dense_scalar`.

All evidence is exploratory: one shared AMD EPYC 9R14 KVM host, Rust 1.98.1,
bundled SQLite 3.51.3, deterministic canonical claims with one scope/digest,
and warm OS cache. Repeated calls are process-local subsamples. No confidence
interval, significance, p99, production-readiness, or fleet claim is made.
Prefix admission also changes kernel history; it is not a selectivity-only
treatment. Physical cold storage, mixed classes/scopes/digests, arbitrary
admission histories, independent host/build replication, concurrent updates,
and whole-host allocation/RSS attribution remain unmeasured. ANN was not tested:
exact-path overhead is already large, and ANN would require explicit recall@K
evaluation against the unchanged exhaustive oracle.
