# u0-baseline-measurement

## Discovery trigger

Plan U0 requires retained baseline evidence before the canonicalizer changes.
This record captures the unchanged canonicalizer's allocation, copy, residency,
CPU, and elapsed-time behavior at harness revision
`5b54ecd6bea237e1b7f63ce8ce1ba0fe7fd3f167`. No candidate exists, so no saving,
speedup, or residency change is claimed. Fixture sizes are declared inputs, not
production representativeness.

## Evidence trail

- Raw artifacts: [`runs/u0-baseline-5b54ecd6/`](runs/u0-baseline-5b54ecd6/):
  `run-01.json` .. `run-10.json` (one release process each, every timed sample
  retained) and [`provenance.json`](runs/u0-baseline-5b54ecd6/provenance.json).
- Driver: `crates/daemon/examples/canonical_output_evidence.rs`, built with
  `--release --features bench-internals,test-support --locked`.
- Schedule: `scripts/perf/canonical-output-paired-runs.sh baseline 5b54ecd6 <dir>`.
  The `paired` mode of the same script freezes the ten-pair AB/BA order for the
  candidate comparison: odd pairs run A then B, even pairs run B then A, each
  run a fresh process whose fixtures are prepared before its timed cells.
- Recorder: `crates/daemon/tests/support/alloc_recorder.rs`, a thread-owned,
  fixed-capacity, non-allocating ledger over `System`.
- Populations: `crates/daemon/tests/support/served_output_fixtures.rs`.
- Tests: `crates/daemon/tests/served_json_passthrough_allocations.rs`.

Provenance: commit `5b54ecd6bea237e1b7f63ce8ce1ba0fe7fd3f167` (clean tree),
`rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1`, features
`bench-internals,test-support`, release profile, Linux 6.12 aarch64, CPU
implementer `0x41` part `0xd40` (ARM Neoverse V1), 32 logical CPUs, allocator
`System` behind the recording wrapper (recording disabled in timing cells),
200 samples per micro cell and 30 per transform cell, 10 processes.

### Seam disposition

`crates/daemon/src/lib.rs` carries `#![forbid(unsafe_code)]`, so no
`GlobalAlloc` can live inside the daemon crate and the in-crate `--lib`
observer route in the S5 evidence is infeasible. The full-constructor observer
instead uses `daemon::transform::served_message_for_test`, a
`#[cfg(feature = "test-support")]` entry that mirrors the existing
`canonical_served_bytes_for_test` pattern and compiles into no production
artifact. This is a test-support seam, not a public constructor API. The
observer records the complete constructor: canonicalizer, block receipts,
SHA-256, identity formatting, and `Arc` conversion.

### Isolation

Recording is thread-owned and gated by a process-wide window mutex. The
`recording_excludes_other_threads_and_tracks_growth_chains` test runs a
concurrently allocating thread during a window and requires zero foreign
events. Both isolated invocations ran and passed on this revision:

```sh
cargo +1.98 test -p daemon --all-features --locked \
  --test served_json_passthrough_allocations \
  full_constructor_observation_covers_receipts_hashing_and_arc_conversion \
  -- --exact --test-threads=1
cargo +1.98 nextest run --profile ci -p daemon --all-features --locked \
  --test served_json_passthrough_allocations --test-threads 1 \
  -E 'test(=full_constructor_observation_covers_receipts_hashing_and_arc_conversion)'
```

### Allocation ledgers

Ledgers are identical across all ten processes. Sizes are requested layout
sizes. A `realloc` event is an allocator request, not a physical copy count.
"Logical reorder bytes" is derived from return provenance: a fresh exact-size
return buffer means the reorder copier materialized N bytes into B; the
serialization buffer's own growth chain means zero.

| Population | Class | N (bytes) | Return provenance | cap/len | Output-sized allocs | Logical reorder bytes | Canonicalizer events (alloc+realloc / realloc / dealloc) | Canonicalizer requested | Canonicalizer peak | Constructor events | Constructor requested | Constructor peak | Peak/N |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `retained_ascii/1blocks` | canonical-miss | 137 | fresh exact-size (B) | 1.00 | 1 | 137 | 14 / 6 / 7 | 1409 | 1033 | 22 | 2265 | 1033 | 7.54 |
| `retained_escaped/1blocks` | canonical-miss | 141 | fresh exact-size (B) | 1.00 | 1 | 141 | 28 / 7 / 20 | 2125 | 1394 | 36 | 2981 | 1394 | 9.89 |
| `retained_large_payload/1blocks_65536B` | canonical-miss | 65598 | fresh exact-size (B) | 1.00 | 1 | 65598 | 11 / 4 / 6 | 262829 | 197176 | 21 | 525800 | 197176 | 3.01 |
| `typed_shell/1blocks` | unordered-miss | 85 | fresh exact-size (B) | 1.00 | 1 | 85 | 15 / 4 / 10 | 1027 | 822 | 24 | 1840 | 822 | 9.67 |
| `one_edited_block/1blocks` | unordered-miss | 78 | fresh exact-size (B) | 1.00 | 1 | 78 | 15 / 4 / 10 | 1006 | 808 | 24 | 1804 | 808 | 10.36 |
| `retained_ascii/65blocks` | canonical-miss | 7177 | fresh exact-size (B) | 1.00 | 1 | 7177 | 281 / 81 / 199 | 75201 | 50665 | 417 | 99481 | 50665 | 7.06 |
| `retained_escaped/65blocks` | canonical-miss | 7437 | fresh exact-size (B) | 1.00 | 1 | 7437 | 1191 / 146 / 1044 | 121741 | 63405 | 1327 | 146277 | 63405 | 8.53 |
| `retained_large_payload/65blocks_65536B` | canonical-miss | 4262142 | fresh exact-size (B) | 1.00 | 1 | 4262142 | 151 / 16 / 134 | 21014201 | 12677278 | 417 | 38076276 | 12677278 | 2.97 |
| `typed_shell/65blocks` | unordered-miss | 3212 | fresh exact-size (B) | 1.00 | 1 | 3212 | 282 / 15 / 266 | 58088 | 39822 | 483 | 79300 | 39822 | 12.40 |
| `one_edited_block/65blocks` | unordered-miss | 7118 | fresh exact-size (B) | 1.00 | 1 | 7118 | 1435 / 80 / 1354 | 211182 | 179650 | 1572 | 235404 | 179650 | 25.24 |

Observations at the unchanged canonicalizer:

- Every population, canonical or unordered, returns a fresh exact-size buffer
  with `capacity == len`: this is B. Exactly one output-sized allocation is
  recorded per call, and the serialization buffer A is released inside the
  window. For the independently established canonical misses this is the one
  B allocation and N logical reorder-output bytes that plan R1 targets.
- The escaped-key population pays decoded-key allocations in `sort_fields`
  (1191 canonicalizer events at 65 blocks against 281 for ASCII keys). That
  cost is expected and not promised to disappear.
- The full-constructor peak equals the canonicalizer peak in every cell; A and
  B are live simultaneously before A is released, and later receipts, hashing,
  and the `Arc` payload stay below that peak. For the large-payload cells the
  peak is about 3x N (A grown past N, B at N, plus the retained `Value`
  payload copy being serialized).
- Peak/N is dominated by fixed metadata for small outputs and is not a
  residency bound for other shapes.

### Served-order and cache frequencies

Classification uses the independent oracle: a served message is canonical
order when its declaration-order serialization equals its canonical bytes.
Counts are identical across all ten processes.

| Workload | Served | Canonical order | Noncanonical order | Cache hits | Cache misses | Dirty skips |
|---|---|---|---|---|---|---|
| `transform_cold/100msgs_2KiB_mixed` | 102 | 100 | 2 | 0 | 102 | 0 |
| `transform_warm/100msgs_2KiB_mixed` | 102 | 100 | 2 | 102 | 0 | 0 |
| `transform_cold/1000msgs_2KiB_mixed` | 1002 | 1000 | 2 | 0 | 1002 | 0 |
| `transform_warm/1000msgs_2KiB_mixed` | 1002 | 1000 | 2 | 1002 | 0 | 0 |

In the cold pass every served message is a cache miss: 100 or 1000 canonical
misses (retained-original ingress) plus 2 unordered misses (the synthetic
`m0`/`m1` typed shells). In the warm pass every served message is a positive
hit and no canonicalizer runs. Completed-output page replay is a host-level
`PreparedOutput` reuse path that this in-process driver does not construct;
it is distinct from the warm item-cache hit above.

### Timing

Each cell reports per-call elapsed samples inside one process. The table shows
the median over the ten processes of each process's p50 and p95, with the
min..max across processes, and CPU time per sample from the process clock.
These are in-process boundaries, not host latency.

| Boundary | Population | Samples/run | p50 ns (median over runs, min..max) | p95 ns (median over runs, min..max) | CPU ns/sample |
|---|---|---|---|---|---|
| canonicalizer | `retained_ascii/1blocks` | 200 | 1198 (1180..1237) | 1262 (1235..1288) | 1267 |
| full_constructor | `retained_ascii/1blocks` | 200 | 4316 (4291..4405) | 4464 (4427..4540) | 5231 |
| canonicalizer | `retained_escaped/1blocks` | 200 | 1962 (1944..1996) | 2099 (2085..2120) | 2027 |
| full_constructor | `retained_escaped/1blocks` | 200 | 5143 (5082..5200) | 5266 (5232..5338) | 6125 |
| canonicalizer | `retained_large_payload/1blocks_65536B` | 200 | 30727 (30608..34571) | 30835 (30702..34643) | 30951 |
| full_constructor | `retained_large_payload/1blocks_65536B` | 200 | 459086 (457948..460569) | 465554 (464062..466244) | 460927 |
| canonicalizer | `typed_shell/1blocks` | 200 | 920 (912..928) | 977 (962..988) | 974 |
| full_constructor | `typed_shell/1blocks` | 200 | 2567 (2545..2587) | 2640 (2617..2660) | 2798 |
| canonicalizer | `one_edited_block/1blocks` | 200 | 902 (896..908) | 947 (936..959) | 958 |
| full_constructor | `one_edited_block/1blocks` | 200 | 2544 (2537..2559) | 2608 (2599..2632) | 2780 |
| canonicalizer | `retained_ascii/65blocks` | 200 | 57866 (57541..58683) | 59490 (59073..60298) | 59133 |
| full_constructor | `retained_ascii/65blocks` | 200 | 227933 (226905..228781) | 236630 (233403..238719) | 283255 |
| canonicalizer | `retained_escaped/65blocks` | 200 | 103991 (103249..104355) | 105311 (104267..108241) | 105506 |
| full_constructor | `retained_escaped/65blocks` | 200 | 293128 (291330..295691) | 298603 (297644..301899) | 336984 |
| canonicalizer | `retained_large_payload/65blocks_65536B` | 200 | 1970785 (1933205..2147438) | 2251959 (2014774..2365426) | 2012497 |
| full_constructor | `retained_large_payload/65blocks_65536B` | 200 | 34028410 (33915968..34109617) | 34269845 (34072437..34370334) | 35177531 |
| canonicalizer | `typed_shell/65blocks` | 200 | 28780 (28632..28882) | 29121 (29000..34769) | 29111 |
| full_constructor | `typed_shell/65blocks` | 200 | 101042 (100751..101495) | 101866 (101209..104977) | 103486 |
| canonicalizer | `one_edited_block/65blocks` | 200 | 122165 (121272..122733) | 127237 (126536..127777) | 122919 |
| full_constructor | `one_edited_block/65blocks` | 200 | 266270 (264805..267926) | 272191 (271276..273949) | 288789 |
| transform_cold | `100msgs_2KiB_mixed` | 30 | 4158135 (4127650..4195048) | 4218580 (4191159..4306033) | 4380731 |
| transform_warm | `100msgs_2KiB_mixed` | 30 | 2819670 (2787672..2862211) | 2898922 (2870088..2945167) | 2976377 |
| transform_cold | `1000msgs_2KiB_mixed` | 30 | 35923485 (35780715..36194779) | 36320035 (36151040..36757424) | 40305932 |
| transform_warm | `1000msgs_2KiB_mixed` | 30 | 22348216 (22114128..22746850) | 23229345 (22422200..23568659) | 24626926 |

Timing boundaries: `canonicalizer` covers serialization, span sorting, and the
reorder copy through `canonical_served_bytes_for_test`; `full_constructor`
adds block receipts, SHA-256, identity formatting, and `Arc` conversion;
`transform_cold` runs `transform_cached` with a fresh output cache per call;
`transform_warm` runs it with a primed cache. Transport publication is outside
every cell.

### Host latency

Unmeasured. No real-host transform driver, including `serve_native`, is
retained by this revision, and no request-send-to-matching-terminal timing
exists. The in-process transform cells above are not user-visible latency and
the `ipc_budget` echo benchmark is only a transport control.

## Failure scenario

A candidate can pass byte checks while allocating a replacement output-sized
scratch buffer, shrinking A, or keeping a full copy. The provenance
classification in `canonical_miss_return_buffer_provenance_is_classified`
rejects each: the returned buffer must come from A's own growth chain, stay
live through return, and no other fresh exact-N allocation may exist.

## Timing windows and dependencies

Allocation windows start before `canonical_served_bytes_for_test` or
`served_message_for_test` and end at their return; reference bytes, fixture
construction, and assertions run outside. Timing cells drop each returned value
after the sample's clock stops and use no allocation recording.

## What a test must construct

The candidate run must use `scripts/perf/canonical-output-paired-runs.sh paired
5b54ecd6 <candidate>` so the baseline binary is rebuilt from this harness
revision and rerun beside the candidate on the frozen schedule. The
`expected_return_provenance` table in `served_output_fixtures.rs` must then
change to `SerializationGrowthChain` only for the three canonical populations;
the typed and one-edited-block populations remain B.

## Investigation log

### Q: Does the full-constructor peak differ from the canonicalizer peak?

- Sources examined: the ten baseline ledgers above.
- Findings: identical in every cell; A and B coexist before A is released and
  nothing later exceeds that. A candidate that returns A with slack will move
  the peak to A's capacity plus the retained payload; measure it rather than
  assume it falls.
- Missing evidence: candidate ledgers.
- Conclusion: unresolved, needs the U1 paired run.

### Q: What proportion of the declared workloads are canonical misses?

- Sources examined: the frequency table above.
- Findings: 100 of 102 and 1000 of 1002 cold served messages are canonical
  order; the two synthetic typed shells are unordered. Warm passes hit on all.
- Missing evidence: any production population; these fixtures are controlled.
- Conclusion: resolved for the declared fixtures only.
