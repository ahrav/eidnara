# Benchmark baselines for the Transform Edit Responses work

## Evidence status

This file retains historical measurement evidence from the full implementation
snapshot. The corrected client harness has a local exploratory baseline against
`53f803af774c16ddb024b5f6a11576a243740fea`. It is not an end-to-end benchmark or
a U5 acceptance result. No candidate comparison is reported here.

The source-guard precursor starts at
`352ce13fdac3024485e7d05c1679fe0db74c8d98`, which includes independent Rust
changes. That revision is not the measured old-code artifact. The retained
baseline must not be relabelled as a measurement of this precursor's parent.
The harness is copied unchanged from the donor; the receipts below describe
the historical run, not checks newly executed for this precursor.

The old client measurements are invalid for inference. Their harness started
all four cases eagerly, accepted unchanged raw output through a length-only
check, and read request inspection objects instead of authoritative serialized
text. It did not decode native continuation items. The old pre/post table and
per-walk timing claims are withdrawn. The archived client summaries also differ
from that table and contain no per-sample observations, so their provenance
cannot repair the comparison.

The original files remain in `/tmp/opencode/eidnara/baseline/`:
`client-bench.pre-533.json`, `client-bench.post-533.json`,
`hot_path_e2e.pre-533.log`, `hot_path_e2e.log`, and `rev.txt`. The historical
verification receipt checks their SHA-256 values. These host-local artifacts
are not checked into the repository.

## Daemon hot path (Criterion, `pre-533` saved baseline)

These historical Criterion estimates remain descriptive daemon-only evidence.
The retained log confirms the values below; this harness repair does not rerun
them or establish paired inference. Do not add these estimates to client
quantiles to claim end-to-end latency.

`cargo +1.98 bench -p daemon --bench hot_path --features bench-internals --locked -- e2e --save-baseline pre-533`

| Benchmark | Time (lower, estimate, upper) |
| --- | --- |
| e2e/first_hard/100msgs_2KiB_mixed | 6.7271 ms, 6.7818 ms, 6.8465 ms |
| e2e/first_hard/1000msgs_2KiB_mixed | 61.584 ms, 61.668 ms, 61.775 ms |
| e2e/steady/100msgs_2KiB_mixed | 3.9180 ms, 3.9293 ms, 3.9440 ms |
| e2e/steady/1000msgs_2KiB_mixed | 26.045 ms, 26.251 ms, 26.403 ms |
| e2e/steady/1000msgs_2KiB_prose | 24.946 ms, 25.146 ms, 25.300 ms |
| e2e/steady/1000msgs_2KiB_code | 25.972 ms, 26.088 ms, 26.222 ms |
| e2e/steady/1000msgs_2KiB_json_tool | 28.051 ms, 28.234 ms, 28.393 ms |
| e2e/steady_output_cache/1000msgs_2KiB_mixed | 15.950 ms, 16.013 ms, 16.100 ms |
| e2e/steady_caveman/1000msgs_2KiB_mixed | 31.621 ms, 31.780 ms, 31.888 ms |

## Corrected client baseline

The historical source snapshot is `/tmp/opencode/transform-client-53f803af/`,
extracted with `git archive 53f803af774c16ddb024b5f6a11576a243740fea`. Only
`packages/opencode-plugin/scripts/bench-transform-client.ts` is overlaid.
Root and plugin `node_modules` in that historical snapshot are symlinks to the
existing workspace dependency directories. This describes the old run, not
dependency setup for the source-guard worktree, which installs its own links.
No Rust build or build-directory copy is needed. The historical artifact check
compares all 1,993 archived files byte-for-byte and verifies that the overlaid
harness matches the workspace harness.

| Identity | Value |
| --- | --- |
| Source revision | `53f803af774c16ddb024b5f6a11576a243740fea` |
| Source archive SHA-256 | `a6a77a9b9b07a29a6cfc6b0d46ec8b29f0f041451ee79197cae05bc1cef12389` |
| Harness SHA-256 | `095dd540ec2ca006903cc1cae3fa9b53796740aec4110b0d59ef8a12d730db6b` |
| Lockfile SHA-256 | `db879c1b7583856c6b875d4b541c3e417d34dca4684f58f0ad8d6ae3159f0e5b` |
| Runtime | Bun 1.3.14, Linux x64 |
| CPU / kernel | AMD EPYC 9R14 / `6.12.103-127.188.amzn2023.x86_64` |

### Fixture, timing, and oracle

The large fixture has 1,000 alternating user/assistant messages, each with
2,048 ASCII text bytes. The small fixture has 8 messages of 256 bytes. Text
repeats `alpha beta gamma delta epsilon zeta eta theta `; IDs, session IDs,
roles, and assistant model fields are deterministic. Warm passes append one
message to raw input after a successful cold prime. The fixture hash covers
the complete submitted native array for each pass. This corpus is distinct
from the old per-message word-rotation generator.

Cases run in this order: large cold, large warm append, small cold, small warm
append. Each case has 3 discarded warmup instances and 30 measured instances.
Each instance owns a fresh transform and clears its session afterward. Warm
instances each prime their own cache. All cases and instances are sequential
in one Bun process. The 30 observations are not independent paired experiment
units. Reported p95 is an interpolated descriptive quantile, not a confidence
bound.

The timer encloses the complete `run()` call, including its guards, sizing,
ordinal work, CK projection, request serialization, application, and publication
where that artifact implements them. It also includes fake-client request JSON
decoding, native page assembly, response JSON encoding, and response parsing.
Fixture generation, expected-output construction, equality checks, memory
observations, and session clear are outside the timer. Oracle allocations can
still affect process memory and later garbage collection.

The fake adds `benchmarkPublished: true` to the first native message. Expected
output is constructed independently from the fixture, not from the fake's
response. Every prime and timed pass must publish the complete expected array,
preserve the host array identity, leave the input unchanged, complete exactly
one logical response without retries, take the expected full/delta path, and
leave the transform's consecutive-failure count at zero. A raw fail-open pass
cannot pass this oracle, even if its length is correct.

The fake reads `serializedJsonText`, validates page order and completion, and
reassembles native continuation chunks before responding. It uses the final
page's tail-delta metadata. Intermediate responses use `{ staged: true }` and
count toward response bytes. It does not implement CK policy, CK continuation
assembly, digest validation, note delivery, or a real shared-memory transport.
Its native-only echo is not U5's combined CK and native daemon baseline or the
complete suffix-comparator experiment.

### Measurements

Historical working directory:
`/tmp/opencode/transform-client-53f803af/packages/opencode-plugin`.

```sh
BENCH_SOURCE_REVISION=53f803af774c16ddb024b5f6a11576a243740fea \
  bun scripts/bench-transform-client.ts --samples 30 --warmup 3 \
  --messages 1000 --bytes 2048 \
  --json /tmp/opencode/eidnara/baseline/sequential-v1/client-old.json
```

| Case | Median (ms) | Descriptive p95 (ms) | Median fake response bytes | Timed request pages |
| --- | ---: | ---: | ---: | ---: |
| large-cold | 85.446 | 92.892 | 2,185,060 | 9 |
| large-warm-append | 1.414 | 1.760 | 4,585 | 1 |
| small-cold | 0.449 | 0.671 | 3,175 | 1 |
| small-warm-append | 0.241 | 0.396 | 993 | 1 |

JSON retains all 132 observations, including warmups, plus 66 cold primes.
Each includes elapsed time, input hash, request and response bytes, page/chunk
counts, delta count, and `process.memoryUsage()`. These are memory snapshots
after the oracle, not peak memory, allocation counts, or retained logical bytes.
Those latter measures are unavailable from this harness.

### Historical verification and evidence

Evidence directory: `/tmp/opencode/eidnara/baseline/sequential-v1/`.

- `client-old.json` and `client-old.log` contain the baseline above. JSON SHA-256:
  `7db3e62bb8487429d0b6bd7ac63aee2e3625f9fa453ccbd5d4e8c90ad1d2f209`.
- `smoke-linked.json` and `.log` verify the old artifact with 1 warmup and 1
  measured instance per case before collection. `smoke.log` retains the failed
  import attempt before the plugin dependency symlink was added.
- `harness-check.ts` and `harness-check-fixed.log` retain executable controls.
  `schedule` observes 18 runs, peak concurrency 1, and four contiguous cases.
  `raw`, `warm-raw`, and `content` reject full raw output, warm raw output, and
  tail corruption. `missing` rejects a no-dispatch pass. `inspection` succeeds
  after only the request's inspection view is changed. `harness-check.log`
  retains the initial test-driver error before its mock factory was corrected.
- `continuations.json` and `.log` use `--messages 2 --bytes 600000 --samples 1
  --warmup 1`. Both large cold and large warm passes assemble 20 native chunks
  over 8 pages and pass the full publication oracle. This is a paging check,
  not a replacement for the required 1,000-message measurement.
- `verify-artifact.py` and `artifact-verification.log` verify source isolation,
  harness identity, preserved archives, sample counts, and paging markers.
- `biome.log` and `typecheck.log` record passing scoped harness formatting/lint
  and plugin script typechecking in the historical workspace. The repository
  comment marker scan and the two-file `git diff --check` also pass. No
  whole-repository build, native-addon build, bundle smoke, or Rust checks run
  for that repair. `typecheck-node24.log` records a second passing script
  typecheck using Node 24.18.0. Successful typechecks produce no output.

Historical replay commands from the repository root:

```sh
for scenario in schedule raw warm-raw content missing inspection; do
  bun /tmp/opencode/eidnara/baseline/sequential-v1/harness-check.ts "$scenario"
done
python /tmp/opencode/eidnara/baseline/sequential-v1/verify-artifact.py
```

Scoped checks from `packages/opencode-plugin`:

```sh
bun ../../node_modules/.bin/biome check scripts/bench-transform-client.ts
/home/ahrav/.local/share/mise/installs/node/24.18.0/bin/node \
  ../../node_modules/typescript/bin/tsc -p tsconfig.scripts.json
```

The donor records writer verification against code and retained evidence.
It also records that a fresh documentation verifier could not start because
the session's subagent depth limit was reached. Independent documentation
verification remains open; copying this evidence does not complete it.

### Runtime and remaining U5 work

CI's `gates` job selects Node 24; `native-addon` selects Node 24.18.0. The plugin
declares Node `>=24.15.0`. Node-based checks use the existing mise install at
`/home/ahrav/.local/share/mise/installs/node/24.18.0/bin/node` by prepending its
directory to PATH. The client baseline itself runs on Bun.

U5 still requires fixed three-artifact paired measurements, alternating order,
confidence intervals, real daemon/transport coverage, separated stage timings,
allocation and retention evidence, all-changed and both-source controls,
early/scattered changes, eviction, cancellation, and churn workloads. This
local baseline cannot establish byte-reduction, latency-regression, or bounded
retention acceptance, and cannot be promoted into that evidence after the fact.
