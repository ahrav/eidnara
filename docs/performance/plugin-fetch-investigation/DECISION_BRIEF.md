# Decision brief: the plugin fetches the full memory surface before matching

Revision `8e0491225a7292ef077c675d44b94f94a24041d3`. Research log:
`RESEARCH_LOG.md` in this directory. No production code was changed or
committed; two measurement patches live under `evidence/` and were reverted.

## Verdict

**Pursue the redesign: a query-aware `kernel.read` variant that ranks on the
daemon and returns only the authorized top-k.** Ship the typed response
encoder as an independent local fix as well; it is small, byte-identical, and
helps every remaining whole-surface reader (sidebar count poll, memory tool).

The scout's hypothesis holds: the plugin's explicit search pays for the whole
visible surface on every non-id query, hit or miss, and the cost is linear in
surface size at every server stage and on the client.

## Reproduced baseline

| Measurement | Result |
|---|---|
| Bench cell read/1000-rows/narrow-scope (10 s loop, two reps) | 19.96–20.37 ms/request |
| read/10-rows/narrow-scope | 0.33 ms |
| read/1000-rows/wide-scope (17 served of 1,000) | 2.49 ms |
| Client fixture, 1,000 rows (673,771 B): `JSON.parse` / rank / parse+rank+pack | 1.11–1.16 / 0.33–0.42 / 1.51–1.62 ms |
| Client fixture, 8,192 rows (5,543,475 B): parse / rank / total | 9.7–10.0 / 4.1 / 12.2–13.3 ms |
| Route at the 8,192-row cap (scout fixture, 5,285,046 B) | 204–220 ms |

## Limiting mechanism

Two halves, sequential within one request (perf by thread: 50.7% runtime
thread, 49.3% blocking pool at 1,000 rows; 58.9% / 41.0% at 8,192 rows):

1. **SQLite, ~10.4 ms at 1,000 rows.** The served-class join
   (`visible_as_of_in_scope`) costs 7.3–7.9 µs per registry row (7.8 ms) and
   is 40% of the route; payload sizes + hydration add 2.5 ms. This half runs
   over two pooled reader connections, so it also caps throughput: eight
   concurrent callers reach only 2.4× the single-caller rate.
2. **JSON, ~7.3 ms at 1,000 rows.** `json!` tree construction (2.1 ms), a
   `to_value(&rows)` deep copy of every row tree inside the response `json!`
   (1.9 ms), tree drops (~1.2 ms), and three serialization passes (row
   measure 0.46, whole measure 0.42, write 0.55). Allocator functions are
   24% of all samples at 1,000 rows and rise with size
   (`malloc_consolidate` alone 6.8% at 8,192 rows).

Then the client parses the whole surface (1.1 ms per 1,000 rows) and ranks it
(0.4 ms). Matching is the cheapest part of the whole path.

The same whole-surface read runs every 2 s per watched session for the
sidebar's memory count (`rpc-handlers.ts:804,441`), so the cost is not paid
only on explicit searches.

## Headroom

| Design | Server, 1,000 rows | Server, 8,192 rows | Client | Scales with |
|---|---:|---:|---:|---|
| Current | 19.3 ms | 204–220 ms | 1.5 / 13 ms parse+rank | rows × (join + JSON + bytes) |
| Typed encoder (measured A/B) | 14.0–14.6 ms (−26%) | ~150 ms est. (−28% of JSON half) | unchanged | rows × (join + typed encode + bytes) |
| Pushdown, no index (measured prototype) | 1.1–1.4 ms hit, 0.4–0.7 ms miss | 6.8–9.3 ms hit, 3.4–5.7 ms miss | ~0.05 ms (k rows) | rows × ~0.6–0.9 µs scan + 0.2–0.5 ms authorize |
| Pushdown + substring index (model, not built) | ~0.3–0.7 ms | ~0.3–0.7 ms | ~0.05 ms | candidates; adds write-path and schema cost |

Local optimization can remove at most the JSON half (~7–10 ms of ~19); it
cannot touch the join, the transfer, or the client parse. The redesign removes
94–97% of the server time and the client parse on every fixture measured.

## Experiment table

| # | Hypothesis | Change | Result | Correctness / quality | Keep? |
|---|---|---|---|---|---|
| 0 | Scout numbers reproduce | none | route 19.96–20.37 ms; client 1.5 ms | — | baseline |
| 1 | Cost splits into SQLite vs JSON halves | stage replica in `read_route_scout` | join 7.8, sizes 0.9, hydrate 1.6, JSON 7.3 ms | replica bytes == route bytes | attribution kept |
| 2 | Stages scale linearly | 256–12,000 rows, wide scope | 7.3–7.9 µs/row join; JSON ~7 µs/row; 220 ms at cap | — | kept |
| 3 | Rank-then-authorize pushdown preserves top-k and removes most cost | scan + rank in Rust + Named-ids authorize + typed encode of k | 1.1 ms hit / 0.4 ms miss (1,000); 6.8 / 3.4 ms (8,192) | ordered ids == client ranker on every fixture incl. hidden rows and 1 KB payloads; bun vs Rust lowercase identical except 55 Unicode 16/17 code points | **keep (recommended)** |
| 4 | Typed encoder cuts route ~30% | `evidence/typed-read-body.patch` (Emit source) | −26% at 1,000 and 10 rows, −2% wide (3 interleaved A/B rounds) | byte-identical body; 73 route tests + 14 dispatch tests pass | keep (complementary) |
| 5 | Skipping only the `json!` deep copy | `evidence/no-deep-copy.patch` | −14% | byte-identical | superseded by 4 |
| 6 | Whole-surface read is also polled | source trace | sidebar count read every 2 s per session | — | context |
| 7 | Pushdown scales under concurrency | 1/4/8 threads × 10 | route 47→112 req/s (pool-bound); pushdown 775→1,360 req/s | — | kept |
| 8 | Domain filter join is cheap | `object_registry` join in scan | +0.3 µs/row; 1.4 / 9.3 ms totals | parity unchanged | kept |
| — | Substring index needed | not built | scan remainder 3–9 ms at cap | — | deferred |

## Recommended approach

1. **Query-aware read (redesign).** Add a versioned wire request (a `query`
   plus `limit` on `kernel.read`, or a `kernel.search` method) that, inside
   one snapshot: scans live memory-domain decisions in the project's scopes
   (same scope-term subquery the served-class query uses), scores each
   payload with the plugin's exact rule (unique lowercase operands of length
   ≥ 2, matched share, ties by `created_commit_seq` desc then `object_id`),
   authorizes candidates in rank order through the existing `object_ids`
   served-class query in batches until `limit` (+ overflow) visible rows are
   found, hydrates only those, and emits them with the typed encoder. Keep
   the existing id-filtered path and the freshness gate unchanged. Report a
   scan bound explicitly instead of the current "newest 8,192 rows"
   truncation so older hits are no longer silently omitted.
2. **Typed response encoder (local).** Land `evidence/typed-read-body.patch`
   or an equivalent: `PreparedSource::Emit` measured once, written once, no
   `Value` trees, byte-identical output. Benefits the sidebar poll and the
   memory tool, which keep reading whole surfaces.
3. Consider a count-only variant for the sidebar poll separately; it hydrates
   and encodes every row to produce one integer every 2 s.

Tradeoffs: the redesign duplicates the ranking rule in Rust (parity must be
tested, including Unicode lowercasing and anti-memory expiry / exclusion
handling, which today run on the client before ranking); it needs a wire
change; and the scan is still linear in the project's decision count
(~0.6–0.9 µs per row at ~130 B–1 KB payloads). A substring index is the
follow-up if surfaces grow far past the current cap; it is not justified by
the measured remainder and would add a schema element plus write-path work.

## Strongest contrary evidence

- The remaining server cost after the typed encoder is ~14 ms at 1,000 rows,
  which a model tool call does not notice; the redesign's benefit is largest
  at the cap (220 ms → ~7–9 ms) and for the 2 s sidebar poll, whose real
  production sizes are unknown.
- Every measurement is synthetic (tmpfs, warm cache, fixture payloads, a
  2-worker bench runtime). The transport's per-byte cost and open-loop tails
  were not measured.
- 55 Unicode 16/17 code points lowercase differently in Rust than in bun's
  ICU; a server-side ranker cannot be bit-exact with the current client for
  text containing them.

## Reproduction

```
bash docs/performance/plugin-fetch-investigation/reproduce.sh route-read
bash docs/performance/plugin-fetch-investigation/reproduce.sh plugin
bash docs/performance/plugin-fetch-investigation/reproduce.sh profile-read
bash docs/performance/plugin-fetch-investigation/reproduce.sh scout
bash docs/performance/plugin-fetch-investigation/reproduce.sh pushdown-join
bash docs/performance/plugin-fetch-investigation/reproduce.sh concurrency
bash docs/performance/plugin-fetch-investigation/reproduce.sh typed-ab
bash docs/performance/plugin-fetch-investigation/reproduce.sh parity
bash docs/performance/plugin-fetch-investigation/reproduce.sh checks
```

Artifacts: `evidence/` (raw logs, perf self-symbol reports by thread, A/B
summaries, binary SHA-256s, the two reverted patches), `sources/`
(`read_route_scout.rs`, `plugin-client-baseline.ts`, `rank-parity.ts`). The
scout's original evidence is at
`/local/home/ahrav/scratch/eidnara-retrieval-scout-report/`.

## Remaining uncertainty and the smallest validation before implementation

Unknown: production surface sizes, payload sizes, query mix, transport cost,
cold-cache behavior. Smallest validation: implement the pushdown route behind
the existing route test harness with (a) a differential property test that,
for random corpora with Unicode text, hidden rows, and anti-memories, the
server's ordered hits equal `searchKernelMemoryRows` over an unfiltered read
at the same snapshot; (b) one new frozen bench cell (`search/1000-rows`,
`search/8192-rows`) beside the read cells; and (c) one end-to-end plugin call
through the real shm transport timing the whole `eidnara_search` for hit,
miss, and id queries against 1,000 and 8,192 rows. Ship the typed encoder
first if the route change waits; its A/B and tests are already in
`evidence/`.
