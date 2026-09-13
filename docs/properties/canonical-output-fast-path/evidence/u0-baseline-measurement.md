# u0-baseline-measurement

## Discovery trigger

Plan U0 requires retained baseline evidence before the canonicalizer changes.
This record captures the unchanged canonicalizer's allocation, copy, residency,
CPU, and elapsed-time behavior at harness revision `073f578efcb1bb08d54eac3b54c0f837d2f0c857`.
No candidate exists, so no saving, speedup, or residency change is claimed.
Fixture sizes are declared inputs, not production representativeness.

## Evidence trail

- Raw artifacts: [`runs/u0-baseline-073f578e/`](runs/u0-baseline-073f578e/):
  `run-01.json` .. `run-10.json` (one release process each, every timed sample
  retained) and [`provenance.json`](runs/u0-baseline-073f578e/provenance.json).
- Driver: `crates/daemon/examples/canonical_output_evidence.rs`, built with
  `--release --features bench-internals,test-support --locked`.
- Schedule: `scripts/perf/canonical-output-paired-runs.sh baseline 073f578e <dir>`.
  The `paired` mode of the same script freezes the ten-pair AB/BA order for the
  candidate comparison: odd pairs run A then B, even pairs run B then A, each
  run a fresh process whose fixtures are prepared before its timed cells.
- Recorder: `crates/daemon/tests/support/alloc_recorder.rs`, a thread-owned,
  fixed-capacity, non-allocating ledger over `System`.
- Populations and oracles: `crates/daemon/tests/support/served_output_fixtures.rs`.
- Tests: `crates/daemon/tests/served_json_passthrough_allocations.rs`.

Provenance: commit `073f578efcb1bb08d54eac3b54c0f837d2f0c857` (harness sources clean),
`rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1`, requested features
`bench-internals,test-support`, resolved feature lines `daemon v0.1.0
bench-internals,test-support`, `memory-store v0.1.0 test-support`, `serde
v1.0.229 alloc,default,derive,rc,serde_derive,std`, `serde_json v1.0.151
alloc,default,raw_value,std`, release profile, Linux 6.12 aarch64, CPU
implementer `0x41` part `0xd40` (ARM Neoverse V1, from each run file's
`provenance.cpu_model`), 64 logical CPUs, allocator `System` behind the
recording wrapper (recording disabled in timing cells; every allocator call
still pays the wrapper's owner check, so binaries under comparison must share
the recorder source), 200 samples per micro cell and 30 per transform cell,
10 processes. Documents are `canonical-output-evidence/v2`.

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
no-projection constructor arm (`from_message`): canonicalizer, per-block
receipt serialization, SHA-256, identity formatting, and `Arc` conversion. The
arm that reuses projected block receipts is not driven and is marked
unmeasured in the driver output.

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
else is unattributed. "A buffers outside chain" counts buffers outside the
returned chain whose growth chain starts at the capacity `Vec<u8>` gives a
one-byte write and reaches at least N: that root identifies the serialization
buffer A, which begins as `Vec::new()` and receives serde's `{` first. Span
metadata and sort scratch also reach N for small outputs (two to six such
allocations per population) but root at multi-word element counts, so a
size-only count would not isolate A. "A released" reports whether that buffer
was freed inside the window. "Logical reorder bytes" is N when the returned
buffer is B and zero otherwise.

| Population | Class | N (bytes) | Return provenance | cap/len | Exact-N allocs | A buffers outside chain | A released | Logical reorder bytes | Canonicalizer events (alloc+realloc / realloc / dealloc) | Canonicalizer requested | Canonicalizer peak | Constructor events | Constructor requested | Constructor peak | Peak/N |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `retained_ascii/1blocks` | canonical-miss | 137 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 137 | 14 / 6 / 7 | 1409 | 1033 | 22 | 2265 | 1033 | 7.54 |
| `retained_escaped/1blocks` | canonical-miss | 141 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 141 | 28 / 7 / 20 | 2125 | 1394 | 36 | 2981 | 1394 | 9.89 |
| `retained_large_payload/1blocks_65536B` | canonical-miss | 65598 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 65598 | 11 / 4 / 6 | 262829 | 197176 | 21 | 525800 | 197176 | 3.01 |
| `typed_shell/1blocks` | unordered-miss | 85 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 85 | 15 / 4 / 10 | 1027 | 822 | 24 | 1840 | 822 | 9.67 |
| `one_edited_block/1blocks` | unordered-miss | 78 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 78 | 15 / 4 / 10 | 1006 | 808 | 24 | 1804 | 808 | 10.36 |
| `retained_ascii/65blocks` | canonical-miss | 7177 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 7177 | 281 / 81 / 199 | 75201 | 50665 | 417 | 99481 | 50665 | 7.06 |
| `retained_escaped/65blocks` | canonical-miss | 7437 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 7437 | 1191 / 146 / 1044 | 121741 | 63405 | 1327 | 146277 | 63405 | 8.53 |
| `retained_large_payload/65blocks_65536B` | canonical-miss | 4262142 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 4262142 | 151 / 16 / 134 | 21014201 | 12677278 | 417 | 38076276 | 12677278 | 2.97 |
| `typed_shell/65blocks` | unordered-miss | 3212 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 3212 | 282 / 15 / 266 | 58088 | 39822 | 483 | 79300 | 39822 | 12.40 |
| `one_edited_block/65blocks` | unordered-miss | 7118 | fresh exact-size (B) | 1.00 | 1 | 1 | yes | 7118 | 1435 / 80 / 1354 | 211182 | 179650 | 1572 | 235404 | 179650 | 25.24 |

Observations at the unchanged canonicalizer:

- Every population, canonical or unordered, returns a fresh exact-size buffer
  with `capacity == len`: this is B. Exactly one exact-N allocation is
  recorded per call, exactly one A is found by its root, and it is released
  inside the window. For the
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
cache for the cold transform) runs before both clocks start, and both the
result and the setup value drop after they stop; the cold transform's
populated cache is therefore torn down outside the clocks. The table shows
the median over the ten processes of each process's p50 and p95, with the
min..max across processes, and CPU time per sample from the process clock.
The CPU window encloses the wall window and its own closing
`clock_gettime`, so it carries a fixed per-sample cost that wall time does
not; "Clock overhead" is that cost measured on an empty call in the same
process, and `CPU - overhead` approximates the call's CPU time. On this host
it is about 460 ns, which is why the 1.3 us canonicalizer cells show 1.7 us
of CPU. These are in-process boundaries, not host latency.

| Boundary | Population | Samples/run | p50 ns (median over runs, min..max) | p95 ns (median over runs, min..max) | CPU ns/sample | Clock overhead ns |
|---|---|---|---|---|---|---|
| canonicalizer | `retained_ascii/1blocks` | 200 | 1252 (1237..1276) | 1297 (1282..1328) | 1731 | 460 |
| full_constructor | `retained_ascii/1blocks` | 200 | 3713 (3690..3732) | 3863 (3850..3883) | 4217 | 462 |
| canonicalizer | `retained_escaped/1blocks` | 200 | 2035 (2011..2064) | 2113 (2086..2148) | 2494 | 463 |
| full_constructor | `retained_escaped/1blocks` | 200 | 4478 (4463..4527) | 4596 (4560..4660) | 4998 | 460 |
| canonicalizer | `retained_large_payload/1blocks_65536B` | 200 | 30673 (30629..30714) | 30773 (30719..30917) | 31260 | 463 |
| full_constructor | `retained_large_payload/1blocks_65536B` | 200 | 453722 (453378..454260) | 460088 (459441..461981) | 455182 | 460 |
| canonicalizer | `typed_shell/1blocks` | 200 | 954 (943..983) | 1005 (994..1036) | 1422 | 466 |
| full_constructor | `typed_shell/1blocks` | 200 | 2614 (2594..2629) | 2679 (2654..2697) | 3096 | 461 |
| canonicalizer | `one_edited_block/1blocks` | 200 | 951 (936..961) | 987 (968..1005) | 1408 | 465 |
| full_constructor | `one_edited_block/1blocks` | 200 | 2607 (2593..2626) | 2661 (2650..2700) | 3080 | 463 |
| canonicalizer | `retained_ascii/65blocks` | 200 | 58322 (57882..58417) | 61078 (59977..61324) | 59220 | 466 |
| full_constructor | `retained_ascii/65blocks` | 200 | 171134 (170207..173391) | 177129 (172666..178680) | 172770 | 465 |
| canonicalizer | `retained_escaped/65blocks` | 200 | 105722 (105420..108054) | 109526 (107328..111392) | 106636 | 473 |
| full_constructor | `retained_escaped/65blocks` | 200 | 221576 (220030..223050) | 227587 (225886..229451) | 223233 | 475 |
| canonicalizer | `retained_large_payload/65blocks_65536B` | 200 | 1926399 (1923222..1928256) | 1940468 (1937433..1943936) | 1928711 | 464 |
| full_constructor | `retained_large_payload/65blocks_65536B` | 200 | 31955496 (31900272..32029733) | 32078715 (32011506..32140599) | 31971434 | 466 |
| canonicalizer | `typed_shell/65blocks` | 200 | 29398 (29311..29517) | 29885 (29510..30167) | 30133 | 473 |
| full_constructor | `typed_shell/65blocks` | 200 | 98082 (97811..99209) | 99349 (98250..103467) | 98875 | 472 |
| canonicalizer | `one_edited_block/65blocks` | 200 | 126505 (125576..127434) | 131109 (129576..132006) | 127683 | 471 |
| full_constructor | `one_edited_block/65blocks` | 200 | 227016 (225770..229012) | 232952 (231630..234973) | 228622 | 471 |
| transform_cold | `100msgs_2KiB_mixed` | 30 | 4136006 (4073686..4210146) | 4180170 (4128662..4258565) | 4130454 | 525 |
| transform_warm | `100msgs_2KiB_mixed` | 30 | 2831173 (2791545..2875771) | 2913043 (2871835..2983989) | 2859894 | 511 |
| transform_cold | `1000msgs_2KiB_mixed` | 30 | 35062108 (34845223..36120763) | 35521043 (35221503..36685876) | 35112512 | 753 |
| transform_warm | `1000msgs_2KiB_mixed` | 30 | 21642765 (21383744..22544600) | 22194527 (21861744..23277735) | 21708342 | 550 |

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
rejects a shrunk or replaced return buffer, an exact-N allocation beside it,
and a second buffer rooted like A that reaches N outside the returned chain:
the returned buffer must be a growth chain that ends at its reported capacity
and stay live through return. Replacement scratch of another shape is not
rejected by these checks. A temporary `Vec<u8>` with capacity `N + 1` that
receives a copy and is dropped before return has neither the required root
nor exact size N and leaves live-at-close unchanged; only the paired ledger
counts (`allocation_events`, `requested_bytes`) would show it as a difference.
General replacement-scratch rejection stays unverified until a negative
control and oracle cover it.
Span metadata that reaches N is not counted, since it roots differently and
is not a copy of the output. Direct pointer identity between the
post-serialization buffer and the returned buffer needs the private
canonicalizer test that plan U1 adds inside `served_json.rs`.

## Timing windows and dependencies

Allocation windows start before `canonical_served_bytes_for_test` or
`served_message_for_test` and end at their return; reference bytes, fixture
construction, and assertions run outside. Timing cells use no allocation
recording.

## What a test must construct

The candidate run must use `scripts/perf/canonical-output-paired-runs.sh paired
<base> <candidate>` where `<base>` is the candidate's merge base, so both
binaries are built from the same recorder, fixtures, and driver and differ
only in the canonicalizer. Pairing against this revision is valid only while
the harness sources are unchanged between it and the candidate. The retained
runs here are absolute ledgers and timings for this revision, not one side of
a paired comparison. The
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
