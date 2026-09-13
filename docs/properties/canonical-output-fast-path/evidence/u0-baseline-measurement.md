# u0-baseline-measurement

## Discovery trigger

Plan U0 requires retained baseline evidence before the canonicalizer changes.
This record captures the unchanged canonicalizer's allocation, copy, residency,
CPU, and elapsed-time behavior at harness revision `c1dafa760b1b48487476d0663586c7936a53b187`.
No candidate exists, so no saving, speedup, or residency change is claimed.
Fixture sizes are declared inputs, not production representativeness.

## Evidence trail

- Raw artifacts: [`runs/u0-baseline-c1dafa76/`](runs/u0-baseline-c1dafa76/):
  `run-01.json` .. `run-10.json` (one release process each, every timed sample
  retained) and [`provenance.json`](runs/u0-baseline-c1dafa76/provenance.json).
- Driver: `crates/daemon/examples/canonical_output_evidence.rs`, built with
  `--release --features bench-internals,test-support --locked`.
- Schedule: `scripts/perf/canonical-output-paired-runs.sh baseline c1dafa76 <dir>`.
  The `paired` mode of the same script freezes the ten-pair AB/BA order for the
  candidate comparison: odd pairs run A then B, even pairs run B then A, each
  run a fresh process whose fixtures are prepared before its timed cells.
- Recorder: `crates/daemon/tests/support/alloc_recorder.rs`, a thread-owned,
  fixed-capacity, non-allocating ledger over `System`.
- Populations and oracles: `crates/daemon/tests/support/served_output_fixtures.rs`.
- Tests: `crates/daemon/tests/served_json_passthrough_allocations.rs`.

Provenance: commit `c1dafa760b1b48487476d0663586c7936a53b187` (harness sources clean),
`rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1`, requested features
`bench-internals,test-support`, resolved feature lines `daemon v0.1.0
bench-internals,test-support`, `memory-store v0.1.0 test-support`, `serde
v1.0.229 alloc,default,derive,rc,serde_derive,std`, `serde_json v1.0.151
alloc,default,raw_value,std`, release profile, Linux 6.12 aarch64, CPU
implementer `0x41` part `0xd40` (ARM Neoverse V1), 32 logical CPUs, allocator
`System` behind the recording wrapper (recording disabled in timing cells),
200 samples per micro cell and 30 per transform cell, 10 processes.

### Seam disposition

`crates/daemon/src/lib.rs` carries `#![forbid(unsafe_code)]`, so no
`GlobalAlloc` can live inside the daemon crate and the in-crate `--lib`
observer route in the S5 evidence is infeasible. The full-constructor observer
uses `daemon::transform::served_message_for_test`, a
`#[cfg(feature = "test-support")]` entry in an integration-test binary that
mirrors the existing `canonical_served_bytes_for_test` pattern and compiles
into no production artifact. It is a feature-gated test-support entry, not a
production constructor API; the ticket's seam decision is recorded here and in
the pull request rather than pre-approved. The observer records the complete
constructor: canonicalizer, block receipts, SHA-256, identity formatting, and
`Arc` conversion.

### Isolation

Recording is thread-owned and gated by a process-wide window mutex. The
`recording_excludes_other_threads_and_tracks_growth_chains` test runs a
concurrently allocating thread during a window and requires zero foreign
events. Both isolated invocations pass on this revision:

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
"Return provenance" classifies the returned buffer from the ledger: a fresh
exact-size allocation is the reorder buffer B; a chain whose every step grows
and that ends at the reported capacity is the serialization buffer A; anything
else is unattributed. "Storage >= N outside chain" counts allocation events of
at least N bytes that are not part of the returned buffer's chain; at the
baseline this is A's own growth, since the returned buffer is B. "Logical
reorder bytes" is N when the returned buffer is B and zero otherwise.

| Population | Class | N (bytes) | Return provenance | cap/len | Exact-N allocs | Storage >= N outside chain | Logical reorder bytes | Canonicalizer events (alloc+realloc / realloc / dealloc) | Canonicalizer requested | Canonicalizer peak | Constructor events | Constructor requested | Constructor peak | Peak/N |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `retained_ascii/1blocks` | canonical-miss | 137 | fresh exact-size (B) | 1.00 | 1 | 3 | 137 | 14 / 6 / 7 | 1409 | 1033 | 22 | 2265 | 1033 | 7.54 |
| `retained_escaped/1blocks` | canonical-miss | 141 | fresh exact-size (B) | 1.00 | 1 | 5 | 141 | 28 / 7 / 20 | 2125 | 1394 | 36 | 2981 | 1394 | 9.89 |
| `retained_large_payload/1blocks_65536B` | canonical-miss | 65598 | fresh exact-size (B) | 1.00 | 1 | 1 | 65598 | 11 / 4 / 6 | 262829 | 197176 | 21 | 525800 | 197176 | 3.01 |
| `typed_shell/1blocks` | unordered-miss | 85 | fresh exact-size (B) | 1.00 | 1 | 6 | 85 | 15 / 4 / 10 | 1027 | 822 | 24 | 1840 | 822 | 9.67 |
| `one_edited_block/1blocks` | unordered-miss | 78 | fresh exact-size (B) | 1.00 | 1 | 6 | 78 | 15 / 4 / 10 | 1006 | 808 | 24 | 1804 | 808 | 10.36 |
| `retained_ascii/65blocks` | canonical-miss | 7177 | fresh exact-size (B) | 1.00 | 1 | 2 | 7177 | 281 / 81 / 199 | 75201 | 50665 | 417 | 99481 | 50665 | 7.06 |
| `retained_escaped/65blocks` | canonical-miss | 7437 | fresh exact-size (B) | 1.00 | 1 | 2 | 7437 | 1191 / 146 / 1044 | 121741 | 63405 | 1327 | 146277 | 63405 | 8.53 |
| `retained_large_payload/65blocks_65536B` | canonical-miss | 4262142 | fresh exact-size (B) | 1.00 | 1 | 1 | 4262142 | 151 / 16 / 134 | 21014201 | 12677278 | 417 | 38076276 | 12677278 | 2.97 |
| `typed_shell/65blocks` | unordered-miss | 3212 | fresh exact-size (B) | 1.00 | 1 | 4 | 3212 | 282 / 15 / 266 | 58088 | 39822 | 483 | 79300 | 39822 | 12.40 |
| `one_edited_block/65blocks` | unordered-miss | 7118 | fresh exact-size (B) | 1.00 | 1 | 3 | 7118 | 1435 / 80 / 1354 | 211182 | 179650 | 1572 | 235404 | 179650 | 25.24 |

Observations at the unchanged canonicalizer:

- Every population, canonical or unordered, returns a fresh exact-size buffer
  with `capacity == len`: this is B. Exactly one exact-N allocation is
  recorded per call, and A is released inside the window. For the
  independently established canonical misses this is the one B allocation and
  N logical reorder-output bytes that plan R1 targets.
- The escaped-key population pays decoded-key allocations in `sort_fields`
  (1191 canonicalizer events at 65 blocks against 281 for ASCII keys). That
  cost is expected and not promised to disappear.
- The full-constructor peak equals the canonicalizer peak in every cell. The
  recorder measures live bytes above the window's start, so the retained
  `Value` payload allocated before the window is excluded. The roughly 3x N
  peak for the large-payload cells is A's grown capacity (at most 2N under
  doubling) plus B at exactly N plus in-window metadata; receipts, hashing,
  and the `Arc` payload stay below that peak.
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

Each cell reports per-call elapsed samples inside one process. Per-sample
setup (the `WireMessage` clone for the full constructor, the fresh output
cache for the cold transform) runs before both clocks start, and each result
drops after they stop. The table shows the median over the ten processes of
each process's p50 and p95, with the min..max across processes, and CPU time
per sample from the process clock. These are in-process boundaries, not host
latency.

| Boundary | Population | Samples/run | p50 ns (median over runs, min..max) | p95 ns (median over runs, min..max) | CPU ns/sample |
|---|---|---|---|---|---|
| canonicalizer | `retained_ascii/1blocks` | 200 | 1264 (1248..1274) | 1305 (1290..1338) | 1738 |
| full_constructor | `retained_ascii/1blocks` | 200 | 3721 (3707..3747) | 3857 (3804..3898) | 4215 |
| canonicalizer | `retained_escaped/1blocks` | 200 | 2056 (2018..2080) | 2128 (2095..2163) | 2521 |
| full_constructor | `retained_escaped/1blocks` | 200 | 4597 (4576..4618) | 4711 (4692..4752) | 5095 |
| canonicalizer | `retained_large_payload/1blocks_65536B` | 200 | 30646 (30606..30688) | 30774 (30716..30893) | 31198 |
| full_constructor | `retained_large_payload/1blocks_65536B` | 200 | 454095 (453808..455773) | 460127 (459015..461334) | 455811 |
| canonicalizer | `typed_shell/1blocks` | 200 | 930 (923..943) | 976 (968..999) | 1389 |
| full_constructor | `typed_shell/1blocks` | 200 | 2579 (2569..2589) | 2635 (2617..2647) | 3060 |
| canonicalizer | `one_edited_block/1blocks` | 200 | 923 (920..933) | 965 (959..974) | 1381 |
| full_constructor | `one_edited_block/1blocks` | 200 | 2560 (2550..2579) | 2618 (2599..2629) | 3056 |
| canonicalizer | `retained_ascii/65blocks` | 200 | 59428 (59009..59899) | 62390 (61260..64017) | 60459 |
| full_constructor | `retained_ascii/65blocks` | 200 | 172035 (170749..173133) | 178117 (175318..179222) | 173711 |
| canonicalizer | `retained_escaped/65blocks` | 200 | 104524 (103706..105431) | 109689 (108658..111057) | 105674 |
| full_constructor | `retained_escaped/65blocks` | 200 | 220653 (220352..222271) | 227307 (226722..228863) | 222579 |
| canonicalizer | `retained_large_payload/65blocks_65536B` | 200 | 1930026 (1927332..1937992) | 1947352 (1942916..1956609) | 1933368 |
| full_constructor | `retained_large_payload/65blocks_65536B` | 200 | 32068035 (32013302..32200271) | 32133894 (32090048..32280273) | 32076259 |
| canonicalizer | `typed_shell/65blocks` | 200 | 28927 (28831..29081) | 29291 (29135..29388) | 29544 |
| full_constructor | `typed_shell/65blocks` | 200 | 98235 (97927..98475) | 101086 (98821..101584) | 99037 |
| canonicalizer | `one_edited_block/65blocks` | 200 | 123820 (123106..125567) | 129221 (128495..130177) | 125146 |
| full_constructor | `one_edited_block/65blocks` | 200 | 225606 (224473..227586) | 231480 (230049..234273) | 227071 |
| transform_cold | `100msgs_2KiB_mixed` | 30 | 4178564 (4115394..4315592) | 4242702 (4191699..4415127) | 4174378 |
| transform_warm | `100msgs_2KiB_mixed` | 30 | 2828422 (2784746..2966361) | 2932633 (2846929..3202072) | 2862874 |
| transform_cold | `1000msgs_2KiB_mixed` | 30 | 35971881 (35687078..36530817) | 36674292 (35889705..36779954) | 36018078 |
| transform_warm | `1000msgs_2KiB_mixed` | 30 | 22462276 (22028001..23293763) | 23000499 (22302677..23714271) | 22487955 |

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
rejects each: the returned buffer must be a growth chain that ends at its
reported capacity, stay live through return, and no allocation of at least N
bytes may exist outside that chain. Direct pointer identity between the
post-serialization buffer and the returned buffer needs the private
canonicalizer test that plan U1 adds inside `served_json.rs`.

## Timing windows and dependencies

Allocation windows start before `canonical_served_bytes_for_test` or
`served_message_for_test` and end at their return; reference bytes, fixture
construction, and assertions run outside. Timing cells use no allocation
recording.

## What a test must construct

The candidate run must use `scripts/perf/canonical-output-paired-runs.sh paired
c1dafa76 <candidate>` so the baseline binary is rebuilt from this harness
revision and rerun beside the candidate on the frozen schedule. The
`expects_serialization_buffer_return` predicate in `served_output_fixtures.rs`
must then return true only for the three retained-original populations; the
typed and one-edited-block populations keep returning B.

## Investigation log

### Q: Does the full-constructor peak differ from the canonicalizer peak?

- Sources examined: the ten baseline ledgers above.
- Findings: identical in every cell; A and B coexist before A is released and
  nothing later exceeds that. A candidate that returns A with slack moves the
  peak to A's capacity plus the retained payload; measure it rather than
  assume it falls.
- Missing evidence: candidate ledgers.
- Conclusion: unresolved, needs the U1 paired run.

### Q: What proportion of the declared workloads are canonical misses?

- Sources examined: the frequency table above.
- Findings: 100 of 102 and 1000 of 1002 cold served messages are canonical
  order; the two synthetic typed shells are unordered. Warm passes hit on all.
- Missing evidence: any production population; these fixtures are controlled.
- Conclusion: resolved for the declared fixtures only.
