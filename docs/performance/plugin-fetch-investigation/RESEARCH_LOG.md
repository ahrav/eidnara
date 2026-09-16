# Research log: plugin fetches the full memory surface before matching

Investigation of the scout finding "Canonical memory search reads and encodes a
whole surface before matching" (scout report at
`/local/home/ahrav/scratch/eidnara-retrieval-scout-report/REPORT.md`, section 1).
The finding is a hypothesis under test, not an established truth.

Rules for this log: every iteration records hypothesis, command, artifact
identity, result, variability, limitation, and keep/discard. Observations,
model predictions, and untested hypotheses stay labeled. No production code
changes; scratch artifacts only.

## Environment

| Item | Value |
|---|---|
| Revision | `8e0491225a7292ef077c675d44b94f94a24041d3` (branch `optimize-plugin-fetch`, clean tree at start) |
| Worktree | `/local/home/ahrav/scratch/eidnara-pr575-repair` |
| Host | Intel Xeon Platinum 8488C, 192 logical CPUs (KVM guest), 2 NUMA nodes, 371 GiB RAM |
| Kernel | Linux 6.12.103-127.188.amzn2023.x86_64 |
| Toolchain | rustc 1.98.0 (88d9e12ae 2026-08-18), LLVM 22.1.8; bun 1.3.14; perf 6.1.186 |
| `/tmp` | tmpfs: fixtures measure warm CPU + SQLite page-cache work, not disk latency |
| Load at start | loadavg 5.45 / 6.49 / 5.09 on a shared development VM; CPU affinity not pinned |
| Shared Cargo target | `/local/home/ahrav/scratch/eidnara/target` (the scout's bench binary `kernel_routes-d9a07286017a717d` is there) |

## Path under test

Plugin: `packages/opencode-plugin/src/tools/eidnara-search/execute.ts:180-191`
issues `client.read({surface: "explicit_search", gated: true})` with no query,
then `searchKernelMemoryRows` (`kernel-memory-search.ts:240-278`) ranks every
returned row by lowercase substring overlap of unique query operands (length
>= 2), ties broken by `created_commit_seq` desc then `object_id`.

Daemon: `crates/daemon/src/kernel_routes/read.rs`

1. `handle_kernel_read` (`:265`): optional gate hop (`tip` + `outbox_lag`, only
   when `gated`), then one `spawn_blocking` hop running `read_visible`.
2. `read_visible` (`:159-248`): `visible_as_of_in_scope` (the large
   `served_classes` join, `crates/kernel/src/admission.rs:3161-3300`), route
   scope filter + `NewestRows` heap (cap 8,192), `decision_payload_sizes_as_of`
   (length only), cumulative-payload cutoff at 8 MiB, then
   `decisions_for_objects_as_of` hydrates the retained payloads.
3. Back on the runtime thread (`:324-347`): `row_json` builds one
   `serde_json::Value` tree per row, `measure_json` serializes each tree once
   into a `CountingWriter` for the byte budget, then `kernel_response` wraps
   the rows in another `Value`.
4. Transport (`dispatch.rs:230-246`, `:335-359`): `PreparedOutput::measure`
   serializes the whole tree a second time (counting), the host reserves
   bytes, then `write_to` serializes a third time into the destination.

Client after the daemon: shm lease → `TextDecoder` → `JSON.parse`
(`host-client/connection.ts:1167-1188`) → `parseReadResponse` validates every
row (`kernel-client/wire.ts:310-343`) → rank → pack.

Note: the kernel_routes bench's `read_request` sets `gated: false`; the plugin
sends `gated: true`. The bench therefore omits one blocking hop plus `tip` and
`outbox_lag`.

## Iterations

(appended below, newest last)

### Iteration 0: reproduce the scout's baseline

Hypothesis: the scout's route and client numbers reproduce on this host at the
same revision with the same binary.

Commands (bench binary `kernel_routes-d9a07286017a717d`, rebuilt by
`cargo bench --locked -p daemon --bench kernel_routes --features test-support --no-run`
into the shared target dir; same hash as the scout's):

```
EIDNARA_KERNEL_ROUTES_PROFILE=read/{10-rows,1000-rows}/narrow-scope <bin>
EIDNARA_KERNEL_ROUTES_PROFILE=read/1000-rows/wide-scope <bin>
bun docs/performance/plugin-fetch-investigation/sources/plugin-client-baseline.ts
perf record -F 99 --call-graph dwarf,8192 -- <bin>   # read/1000-rows/narrow-scope
```

Results (10 s profile loop, approximate mean = 10,000 ms / iterations; two
process repetitions):

| Cell | Iterations (a, b) | ms/request |
|---|---|---|
| read/10-rows/narrow-scope | 30,504 / 29,802 | 0.328 / 0.336 |
| read/1000-rows/narrow-scope | 491 / 501 | 20.37 / 19.96 |
| read/1000-rows/wide-scope (17 served) | 4,022 / 4,017 | 2.49 / 2.49 |

Client fixture (bun, medians of 7×10 calls, three processes), synthetic rows of
the scout's shape (673,771 wire bytes at 1,000 rows; 5,543,475 at 8,192):

| Work | 1,000 rows | 8,192 rows |
|---|---|---|
| `JSON.parse` only | 1.11–1.16 ms | 9.68–10.02 ms |
| rank only (`searchKernelMemoryRows`, common term, k=8) | 0.33–0.42 ms | 4.05–4.13 ms |
| parse + rank + pack, common | 1.51–1.62 ms | 12.24–12.77 ms |
| parse + rank + pack, absent | 1.37–1.46 ms | 12.01–13.28 ms |
| pack only (8 hits) | 0.07 ms | — |

Verdict: reproduced within the scout's ranges. The client fixture's cost is
dominated by `JSON.parse` of the whole surface, not by matching. Evidence:
`evidence/baseline-*.log`, `evidence/baseline-plugin-*.jsonl`,
`evidence/profile-read-baseline.log`.

Profile split by thread (`perf report --sort comm`, 1,343 samples, cycles:u
only — `perf_event_paranoid=2` in this VM, so kernel time such as page faults
is not sampled): main/runtime thread 50.7%, blocking-pool threads 49.3%. The
route awaits the blocking hop before building JSON, so the two halves are
sequential and their CPU shares approximate wall-time shares. Self-symbol
buckets (`evidence/profile-read-baseline-self-by-comm.txt`): SQLite C 29.8%,
main-thread malloc/free/memmove/memcmp 24.4%, serialization into
`CountingWriter` (measurement) 12.0%, blocking-thread libc 11.6%,
`BTreeMap` (`serde_json::Map`) 5.0%, serialization into `BoundedWriter`
(the real encode) 3.4%, rusqlite 2.0%, payload JSON parse 0.5%.

### Iteration 1: stage attribution with a scout example (no production change)

Hypothesis: the route's ~19–20 ms at 1,000 rows splits into a SQLite half
dominated by the served-class join and a JSON half dominated by
`serde_json::Value` tree construction plus three serialization passes.

Method: `sources/read_route_scout.rs`, copied to
`crates/daemon/examples/read_route_scout.rs` for the build only (removed
afterwards), built with
`cargo build --release --locked -p daemon --example read_route_scout` into the
shared target dir. It reuses the bench's `Daemon` fixture verbatim, calls the
real route through `dispatch_value_for_test` for `route/full`, then re-runs
each stage of `read_visible` through the public `KernelStore` API and
reproduces `row_json` / `measure_json` / `kernel_response` / `measure` /
`write_to`. `PARITY json_replica_bytes ok` shows the replica's bytes equal
the real route's bytes, so the replica times the route's own work. 21
repetitions, first dropped, medians reported. Stage sums exclude the
blocking-pool hop, request parsing/binding, `ScopeFilter`, and the drops of
`ReadResponse` and of the pre-copy `rows` vector, which the route also pays.

Results, 1,000 rows narrow scope (`evidence/scout-1000-narrow-a.log`,
`evidence/scout-1000-1scopes-b.log`), route output 640,178 bytes, 1,001 rows:

| Stage | Median ms | Notes |
|---|---:|---|
| route/full (real route via dispatcher) | 19.22 / 19.29 | matches the bench cell |
| stage/tip | 0.014 | |
| stage/visible_as_of_in_scope | 7.77 / 7.82 | `served_classes` join over the registry; 7.8 µs/row |
| stage/newest_heap | 0.20 | |
| stage/payload_sizes | 0.86 | `json_each` IN over 1,000 ids |
| stage/decisions_hydrate | 1.57 / 1.59 | `json_each` IN + payload parse + HashMap |
| json/row_json_trees | 2.08 | 1,000 `json!` trees; `"object": row.object` re-serializes `ObjectRow` into a `Value` |
| json/row_measure | 0.46 | per-row `measure_json` |
| json/wrap_response | 1.95 / 1.85 | `json!({.., "rows": rows})` calls `to_value(&rows)`: a deep copy of every row tree |
| json/measure_whole | 0.42 | second full serialization (counting) |
| json/write_whole | 0.55 | third full serialization (bytes) |
| json/drop_trees | 0.62 / 0.71 | drop of the copied response tree |
| typed/build_rows | 0.42 | borrowed typed rows, no allocation per field |
| typed/row_measure | 0.32 | |
| typed/measure_whole | 0.30 | |
| typed/write_whole | 0.47 | `PARITY typed_bytes ok`: byte-identical to the route |
| rank/common (Rust, 1,000 rows, 2 terms) | 0.95 / 1.03 | char-wise lowercase into scratch; unoptimized |
| rank/absent | 0.89 | a miss scans the same text |
| rank/encode_k_rows (8 rows, typed) | 0.001 | plus ~0.01 ms measure+write |
| candidates/visible_named_8 | 0.17 | `served_classes` with `object_ids` = 8 ids |
| candidates/visible_named_64 | 0.49 | 64 ids: ~7.5 µs per named id, independent of surface size |
| candidates/hydrate_named_8 / _64 | 0.043 / 0.092 | |

Observations:

- SQLite-side stages sum to ~10.4 ms; the served-class join alone is 40% of
  the route. Its cost is per registry row (7.3–7.9 µs/row at 256, 1,000,
  4,000, 8,192, 12,000 rows) plus ~2.3 µs per row the scope subquery
  excludes (wide-scope cell: 2.3 ms to serve 17 rows out of 1,000).
- JSON-side stages the route pays sum to ~6.7 ms plus the untimed drop of the
  pre-copy `rows` vector (~0.6 ms) and of `ReadResponse`. The single largest
  avoidable item is the `to_value(&rows)` deep copy in the `json!` wrapper
  (1.9 ms), followed by tree construction (2.1 ms) and tree drops.
- Three full serializations happen (row measure, whole measure, whole write);
  each pass over `Value` trees costs 0.42–0.55 ms at 1,000 rows. The typed
  encoder does the same three passes in 1.09 ms total and needs no trees.
- A typed encoder replaces ~7.3 ms of main-thread work with ~1.5 ms at 1,000
  rows (route −30%) and ~72 ms with ~11 ms at 8,192 rows (route −28%), with
  byte-identical output and the same measure-before-reserve discipline
  (measurement still retains no bytes).
- The Named-ids form of the served-class query costs 0.17–0.55 ms for 8–64
  ids regardless of surface size. A design that ranks first and authorizes
  only candidates removes the 7.8 µs/row join from the query path.

Verdict: hypothesis confirmed. Both halves matter; neither alone is "the"
bottleneck. Keep both leads.

### Iteration 2: scale sweep of the same stages

Command: `read_route_scout <rows> <scopes> <reps> 8` for
(256,1), (1000,1), (1000,64), (4000,1), (8192,1), (12000,1). Evidence:
`evidence/scout-*-Nscopes-b.log`.

| Rows (served) | route/full | visible join | sizes + hydrate | JSON stages (sum) | typed stages (sum) | rank/common | client parse (bun fixture, similar bytes) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 256 (257) | 5.18 | 2.03 | 0.56 | ~2.0 | ~0.4 | 0.25 | 0.34 |
| 1,000 (1,001) | 19.29 | 7.82 | 2.46 | ~6.7 | ~1.5 | 1.03 | 1.12 |
| 1,000 / 64 scopes (17) | 2.80 | 2.32 | 0.11 | ~0.2 | ~0.03 | 0.02 | — |
| 4,000 (4,001) | 90.2 | 28.2 | 10.7 | ~34 | ~5 | 3.9 | — |
| 8,192 (8,192; 5,285,046 B) | 220.2 | 60.1 | 25.4 | ~72 | ~11 | 8.9 | 9.7–10.0 |
| 12,000 (8,192 cap) | 235.0 | 90.1 | 23.8 | ~68 | ~11 | 9.2 | — |

Observations: at the 8,192-row cap a query costs ~220 ms in the route plus
~13 ms in the client fixture, and a miss pays the same. Beyond the cap the
join still scans every registry row (90 ms at 12,000) while the JSON half is
capped. `perf stat` on a route-only loop at 8,192 rows: 0.34 s sys of 5.3 s,
171k minor faults over 20 calls plus seeding, so kernel time is at most a few
ms per call and does not explain the ~50 ms gap between stage sums and
`route/full` at that size; the gap is consistent with the untimed drops
(`ReadResponse`, pre-copy `rows`) and allocator consolidation
(`malloc_consolidate` is 6.8% of samples at 8,192 rows,
`evidence/profile-read-8192-self-by-comm.txt`, main thread 58.9% / blocking
41.0%).

### Iteration 3: query-aware pushdown without a new index

Hypothesis: ranking on the daemon over the decision payloads, then authorizing
only the ranked candidates through the served-class query in its `object_ids`
form, removes the per-row join, the whole-surface JSON, the transfer, and the
client parse, while producing the same top-k the client ranker produces over
the visible rows.

Method (`sources/read_route_scout.rs`, `Pushdown`, `authorize`): a read-only
rusqlite connection to the fixture's `kernel.sqlite` runs
`SELECT object_id, created_commit_seq, decision_payload FROM decisions WHERE
scope_id=?1 AND created_commit_seq<=?2 AND (invalidated_commit_seq IS NULL OR
?2<invalidated_commit_seq)` (uses `idx_decisions_scope_fk`), parses each
payload with borrowed serde, lowercases with an ASCII fast path (char-wise
`to_lowercase` otherwise), counts matched terms with `memchr::memmem`, sorts
matches by (matched desc, `created_commit_seq` desc, `object_id` asc), then
authorizes candidates in rank order through `visible_as_of_in_scope(ids)` in
batches of 64 until `k` visible rows are found, hydrates those `k` through
`decisions_for_objects_as_of`, and encodes them with the typed encoder. The
scope predicate is the fixture's single project scope; production needs the
served-class query's scope-term subquery instead. `PARITY pushdown_rank_*`
compares the pushdown's ordered ids against the client-semantics ranker over
the route's visible rows.

Results (medians; `evidence/scout-pushdown-*.log`):

| Fixture | route/full | client parse+rank+pack (fixture) | pushdown hit (scan+rank / authorize / hydrate+encode) | pushdown miss |
|---|---:|---:|---:|---:|
| 256 rows | 5.05 | ~0.5 | 0.71 (0.17 / 0.50 / 0.05) | 0.11 |
| 1,000 rows | 18.9 | ~1.5 | 1.11 (0.56 / 0.50 / 0.04) | 0.40 |
| 1,000 rows / 64 scopes (17 in scope) | 2.57 | — | 0.28 (0.03 / 0.21 / 0.04) | 0.02 |
| 8,192 rows | 204 | ~13 | 6.80 (6.13 / 0.61 / 0.07) | 3.40 |
| 1,000 rows, ~1 KB payloads (1,644,284 B body) | 22.7 | — | 1.48 (0.90 / 0.53 / 0.06) | 0.76 |
| 1,000 rows, every third row unadmitted (668 served) | 13.3 | — | 0.99 (0.56 / 0.38 / 0.04) | — |

Ranking parity: `ok` for common, selective, absent, and long queries on every
fixture above, including the unadmitted-row fixture (authorize-after-rank
yields the same top-k as rank-over-visible because authorization is a filter
over a total order). Cross-runtime check: the plugin's `searchKernelMemoryRows`
(bun) over the route's dumped body produced the same ordered ids as the Rust
ranker on the 1,000-row and the 1 KB-payload fixtures
(`sources/rank-parity.ts`, `evidence/rank-parity-ts-1000.log`).

Lowercase parity: bun 1.3.14 `toLowerCase()` and Rust 1.98 `to_lowercase()`
agree on 14 hand-picked samples (final sigma, İ, ẞ, full-width, ligatures;
`evidence/lower-parity-rust.log`) and on every single code point except 55 in
Unicode 16/17 additions (U+1C89, U+A7CB–A7DC subset, U+10D50–10D65,
U+16EA0–16EB8) that Rust lowercases and bun's ICU does not
(`evidence/lowercase-codepoint-diff-bun-vs-rust.txt`). No code point maps to
different non-identity results in the two runtimes. This is a documented
residual parity risk of a server-side ranker, not a blocker.

Scan cost: ~0.4–0.75 µs per row for ~130-byte payloads, ~0.7–0.9 µs per row
for ~1 KB payloads (the ASCII fast path lowercases ~1 MB in well under 1 ms).
The authorize step costs ~0.5 ms for a 64-id batch; a first batch sized to
`2k` would cut it to ~0.2 ms.

Verdict: keep. Pushdown removes 94–97% of the route's time on every fixture
and the client's parse of the whole surface, with no new index and no schema
change, preserving the client ranker's order on the visible set.

### Iteration 4: typed response encoder as a route patch (local optimization)

Hypothesis: replacing the `serde_json::Value` trees and the `json!` deep copy
with typed borrowed rows serialized on demand cuts the route by ~30% at 1,000
rows with byte-identical output.

Method: `evidence/typed-read-body.patch` (applied to `dispatch.rs`,
`kernel_routes/read.rs`, and the bench's `call`, then reverted; not committed).
It adds `PreparedSource::Emit(Arc<dyn Emit>)`, measured once into
`CountingWriter` and written once into the reserved `BoundedWriter`, so
measurement still retains no bytes and the host reservation boundary is
unchanged. `read.rs` measures each typed row for the byte budget exactly as
before (`measure_serialize(&row_out) + 1`), keeps the truncation semantics,
and emits `{gated, known_as_of, rows, state, tip, truncated}` in sorted key
order. Byte parity: the scout's dumped bodies from the unpatched and patched
routes are identical after normalizing the per-run project digest (640,178
bytes each). Tests: `cargo test --release --locked -p daemon --test
kernel_routes` (73 passed, including the row-cap and byte-budget truncation
tests) and `cargo test --release --locked -p daemon --lib dispatch` (14
passed) with the patch applied.

A/B on the frozen bench cells, baseline binary
(`d6c6d709…`, the scout's) vs patched binary (`a4689d9f…`), three interleaved
rounds (`evidence/ab-typed-summary.txt`):

| Cell | baseline ms (r1/r2/r3) | typed ms (r1/r2/r3) | change |
|---|---|---|---|
| read/1000-rows/narrow-scope | 19.27 / 20.20 / 19.34 | 14.04 / 14.56 / 14.62 | −26% |
| read/10-rows/narrow-scope | 0.295 / 0.329 / 0.316 | 0.221 / 0.221 / 0.261 | −25% |
| read/1000-rows/wide-scope (17 served) | 2.49 / 2.49 / 2.49 | 2.40 / 2.48 / 2.45 | −2% |

Verdict: keep as the local-optimization candidate. It removes about a quarter
of the route at 1,000 rows and leaves the served-class join (now ~54% of the
remaining 14.4 ms), the payload-size and hydration queries, the transfer of
the whole surface, and the client parse untouched. It does not change what a
miss costs.

### Iteration 5: the one-line variant (skip the `json!` deep copy only)

Hypothesis: inserting `Value::Array(rows)` into the response map instead of
`json!({.., "rows": rows})` removes the ~1.9 ms `to_value(&rows)` deep copy
and the drop of the copied tree.

Method: `evidence/no-deep-copy.patch` (reverted). Three interleaved rounds,
read/1000-rows/narrow-scope (`evidence/ab3-nocopy-summary.txt`):
baseline 19.27 / 20.20 / 20.24, no-copy 17.76 / 16.69 / 17.01 (−14%), typed
13.62 / 14.60 / 14.58 (−28%).

Verdict: a real but small win; superseded by the typed encoder, which removes
the trees entirely.

### Iteration 6: production frequency of the whole-surface read

Observation (source, not measurement): the sidebar status poll
(`packages/opencode-plugin/src/plugin/rpc-handlers.ts:804`) issues the same
`client.read({surface: "explicit_search", gated: true})` to compute
`memory.rows.filter(isServedMemoryDecisionRow).length` (`:441`), cached for
`RUST_STATUS_CACHE_TTL_MS = 2_000` ms per session and directory. While the TUI
polls, the daemon therefore performs one whole-surface read every 2 s per
watched session, hydrating and encoding every row to produce a count. The
memory tool (`tools/eidnara-memory/execute.ts:352`) also reads the whole
surface (ungated) for its preflights. Explicit search is not the only payer.

### Iteration 7: concurrency (closed loop, 1,000 rows)

Method: `SCOUT_CONCURRENCY=1 read_route_scout 1000 1 6 8`. Each thread issues
10 requests; wall time per round includes thread creation and, for the
pushdown, a per-thread read-only connection open. Route calls run through
`dispatch_value_for_test` on the fixture runtime (2 worker threads); the
kernel's reader pool has `READ_POOL_SIZE = 2` connections
(`crates/kernel/src/open.rs:24`). Medians of 5 warm rounds
(`evidence/scout-concurrency-1000-a.log`):

| Threads × requests | route wall ms (req/s) | pushdown wall ms (req/s) |
|---|---:|---:|
| 1 × 10 | 214 (47) | 12.9 (775) |
| 4 × 10 | 406 (99) | 37.4 (1,070) |
| 8 × 10 | 713 (112) | 58.8 (1,360) |

Observation: the route gains only 2.4× from 8 concurrent callers; its ~10 ms
of SQLite work per request serializes over two reader connections, so the
pool, not CPU, bounds its throughput. The pushdown's authorize step also uses
the pool but holds it ~0.5 ms; the scan used its own connection here and would
use the pool in production (another ~0.6 ms per request), which still leaves
an order of magnitude of headroom.

### Iteration 8: pushdown scan with the memory-domain join

Hypothesis: the pushdown must restrict candidates to memory-domain decisions
before ranking (the client's `isMemoryDecisionRow`), and `decisions` carries
no `domain_id`, so the scan needs an `object_registry` primary-key lookup per
row; this should add well under 1 µs per row.

Method: `SCOUT_SCAN_DOMAIN_JOIN=1` switches the scan to
`FROM decisions d JOIN object_registry o ON o.object_id=d.object_id AND
o.domain_id=?3`. Evidence: `evidence/scout-pushdown-domainjoin-{1000,8192}-a.log`.

| Fixture | scan+rank without join | with join | pushdown total hit / miss with join |
|---|---:|---:|---:|
| 1,000 rows | 0.56 | 0.86 | 1.41 / 0.68 |
| 8,192 rows | 6.13 | 8.65 | 9.33 / 5.72 |

Verdict: +0.3 µs per row; the pushdown stays 13–22× faster than the route at
both sizes. Ranking parity unchanged.

## Rejected or deferred hypotheses

1. **A trigram (or other substring) candidate index is needed to remove the
   full-surface cost.** Deferred, not measured. The scan-based pushdown
   already removes 94–97% of the route and the whole client parse; at the
   existing 8,192-row cap the remaining scan is 3.4–9.3 ms. An index would
   reduce that to sub-millisecond for selective terms (model estimate from
   the Named-ids authorize cost of 0.17–0.55 ms plus verification of ~k
   candidates), but it adds a schema element to a store that rejects schema
   mismatches rather than migrating, write-path maintenance on every commit,
   snapshot-visibility filtering per posting, a fallback for 2-character
   terms, and index-time lowercasing that must match the client. That cost
   is not justified by the measured remainder unless production surfaces
   grow far past the cap or payloads far past ~1 KB.
2. **Client-side matching is the expensive half.** Refuted: matching is
   0.33–0.42 ms at 1,000 rows (4 ms at 8,192); `JSON.parse` of the surface is
   1.1 ms (9.7–10 ms). Both disappear with pushdown.
3. **The byte measurement passes dominate the JSON half.** Refuted in the
   narrow sense: the two counting passes cost 0.46 + 0.42 ms at 1,000 rows;
   tree construction (2.1 ms), the `to_value` deep copy (1.9 ms), and tree
   drops (0.6 ms + the pre-copy vector) are larger. Removing measurement
   alone would not move the route much; removing the trees does (−26%).
4. **SQLite reads are the floor for any design.** Refuted: the served-class
   join is per registry row only because the route authorizes every row
   before ranking. Ranking first and authorizing candidates in the query's
   `object_ids` form costs 0.17–0.55 ms regardless of surface size.
5. **Raising the 8,192-row / 8 MiB caps would improve retrieval quality.**
   Not tested as a change, but the cost curve is linear in rows for every
   server stage (7.3–7.9 µs/row join alone), so raising caps under the
   current design scales the miss cost with them. Pushdown makes the scan
   bound a work bound rather than a transfer bound.

## Open questions and limitations

- Production surface sizes, payload sizes, and query mix are unknown; every
  number above is from synthetic fixtures on tmpfs (warm page cache).
- The plugin↔daemon shm transport cost per byte was not measured in either
  direction; a 640 KB → ~6 KB response can only reduce it.
- Cold-cache behavior was not measured; the scan touches `decisions` pages
  only, the route touches registry, admission, and decisions pages.
- Concurrency was measured on the bench fixture's 2-worker runtime with a
  closed loop; no open-loop tail latency.
- The pushdown prototype filters scope by one `scope_id`; production must use
  the served-class query's scope-term subquery (rows under any scope that can
  match the project, including scopes without a project term).
- Anti-memory expiry (`isExpiredAntiMemoryRow`) and `excludeObjectIds` run on
  the client before ranking today. A pushdown route either replicates them or
  returns ranked hits with overflow (for example up to 32 for `limit` 8) so
  the client can filter and truncate; the encode cost of the overflow is
  ~0.03 ms.
- Rust and bun disagree on lowercase for 55 Unicode 16/17 code points.
