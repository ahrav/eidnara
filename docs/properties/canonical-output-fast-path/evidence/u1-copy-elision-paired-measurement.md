# u1-copy-elision-paired-measurement

## Discovery trigger

Plan U1 returns the serialization buffer A when every object is already in
canonical order, and plan U4 requires the baseline and candidate to run side by
side on the frozen ten-pair schedule before any saving is claimed. This record
compares the U0 harness tip `2a415271` (A: final harness, unchanged
canonicalizer) against candidate
`8a1fb16632684baa175e13097b4a5f91f944c098` (B), which contains the
canonicalizer change and the constructor's early `Arc` conversion.

## Evidence trail

- Raw artifacts: [`runs/u1-paired-2a415271-vs-8a1fb166/`](runs/u1-paired-2a415271-vs-8a1fb166/):
  `pair-01-A.json` .. `pair-10-B.json` and
  [`provenance.json`](runs/u1-paired-2a415271-vs-8a1fb166/provenance.json).
- Schedule: `scripts/perf/canonical-output-paired-runs.sh paired 2a415271 8a1fb166 <dir>`;
  odd pairs run A then B, even pairs run B then A, every run a fresh release
  process built from a detached worktree of its own revision.
- Baseline record: [u0-baseline-measurement](u0-baseline-measurement.md).
  When this record was made it was the `c1dafa76` capture; the A leg here is
  rebuilt from `2a415271` so both legs share one recorder, and its allocation
  ledgers equal that capture. The baseline was later regenerated at `073f578e`
  on a corrected harness (root-identified serialization buffer, setup drops
  outside the clocks, `v2` documents); this record predates that correction
  and has not been rerun on it.
- Production change: `crates/daemon/src/served_json.rs` (`sort_fields` reports
  change, `sort_all_fields` aggregates without short-circuiting, `finalize`
  returns A or copies into B) and `crates/daemon/src/transform.rs`
  (`from_message_reusing` converts the canonical bytes to their `Arc` before
  building block receipts).
- Correctness: `served_json::tests::{every_key_permutation_reports_change_exactly_when_disordered,
  aggregate_order_decision_visits_every_object, unchanged_span_copy_is_identity,
  canonical_input_returns_the_serialization_buffer_and_disordered_input_does_not,
  serialization_error_returns_before_finalization}`,
  `transform::tests::positive_output_cache_hit_reuses_owned_artifacts_without_constructing`,
  and the U0 integration tests with `expects_serialization_buffer_return`
  flipped for the three retained-original populations.

Provenance: harness sources clean at both revisions, `rustc 1.98.1`, features
`bench-internals,test-support` with resolved lines identical for A and B,
release profile, Linux 6.12 aarch64, ARM Neoverse V1 (implementer `0x41` part
`0xd40`), 32 logical CPUs, 200 samples per micro cell, 30 per transform cell.

### Negative control

Forcing the always-copy path (dropping the early return in `finalize`) makes
`canonical_miss_return_buffer_provenance_is_classified` fail on
`retained_ascii/1blocks` with `return-buffer provenance`, and the candidate
passes; the oracle distinguishes the two builds.

### Allocation ledgers (A → B)

Ledgers are identical across the ten processes of each side. "Return" names
the returned buffer: B is a fresh exact-size allocation, A is the serialization
buffer's own growth chain that ends at its reported capacity. Sizes are
requested layout sizes; a `realloc` is an allocator request, not a copy count.

| Population | Class | N | Return | cap/len | Exact-N allocs | Logical reorder bytes | Canonicalizer events | Canonicalizer requested | Canonicalizer peak | Constructor events | Constructor requested | Constructor peak | Peak/N |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `retained_ascii/1blocks` | canonical-miss | 137 | B → A | 1.00 → 1.87 | 1 → 0 | 137 → 0 | 14 → 13 | 1409 → 1272 | 1033 → 928 | 22 → 21 | 2265 → 2128 | 1033 → 928 | 7.54 → 6.77 |
| `retained_escaped/1blocks` | canonical-miss | 141 | B → A | 1.00 → 1.82 | 1 → 0 | 141 → 0 | 28 → 27 | 2125 → 1984 | 1394 → 1394 | 36 → 35 | 2981 → 2840 | 1394 → 1394 | 9.89 → 9.89 |
| `retained_large_payload/1blocks_65536B` | canonical-miss | 65598 | B → A | 1.00 → 2.00 | 1 → 0 | 65598 → 0 | 11 → 10 | 262829 → 197231 | 197176 → 131610 | 21 → 20 | 525800 → 460202 | 197176 → 196818 | 3.01 → 3.00 |
| `typed_shell/1blocks` | unordered-miss | 85 | B → B | 1.00 → 1.00 | 1 → 1 | 85 → 85 | 15 → 15 | 1027 → 1027 | 822 → 822 | 24 → 24 | 1840 → 1840 | 822 → 822 | 9.67 → 9.67 |
| `one_edited_block/1blocks` | unordered-miss | 78 | B → B | 1.00 → 1.00 | 1 → 1 | 78 → 78 | 15 → 15 | 1006 → 1006 | 808 → 808 | 24 → 24 | 1804 → 1804 | 808 → 808 | 10.36 → 10.36 |
| `retained_ascii/65blocks` | canonical-miss | 7177 | B → A | 1.00 → 1.14 | 1 → 0 | 7177 → 0 | 281 → 280 | 75201 → 68024 | 50665 → 43520 | 417 → 416 | 99481 → 92304 | 50665 → 43520 | 7.06 → 6.06 |
| `retained_escaped/65blocks` | canonical-miss | 7437 | B → A | 1.00 → 1.10 | 1 → 0 | 7437 → 0 | 1191 → 1190 | 121741 → 114304 | 63405 → 56274 | 1327 → 1326 | 146277 → 138840 | 63405 → 56274 | 8.53 → 7.57 |
| `retained_large_payload/65blocks_65536B` | canonical-miss | 4262142 | B → A | 1.00 → 1.97 | 1 → 0 | 4262142 → 0 | 151 → 150 | 21014201 → 16752059 | 12677278 → 8415168 | 417 → 416 | 38076276 → 33814134 | 12677278 → 12654480 | 2.97 → 2.97 |
| `typed_shell/65blocks` | unordered-miss | 3212 | B → B | 1.00 → 1.00 | 1 → 1 | 3212 → 3212 | 282 → 282 | 58088 → 58088 | 39822 → 39822 | 483 → 483 | 79300 → 79300 | 39822 → 39822 | 12.40 → 12.40 |
| `one_edited_block/65blocks` | unordered-miss | 7118 | B → B | 1.00 → 1.00 | 1 → 1 | 7118 → 7118 | 1435 → 1435 | 211182 → 211182 | 179650 → 179650 | 1572 → 1572 | 235404 → 235404 | 179650 → 179650 | 25.24 → 25.24 |

Observations:

- Every canonical-miss population now returns A: zero exact-N allocations,
  zero logical reorder bytes, one allocation event fewer, and canonicalizer
  requested bytes fall by exactly N. Canonicalizer peak residency falls by
  less than N or not at all: 105 of 137 bytes for `retained_ascii/1blocks`,
  0 of 141 for `retained_escaped/1blocks`, 7131 of 7437 for
  `retained_escaped/65blocks`, and N minus 32 bytes for the three remaining
  cells. Typed and one-edited-block populations are unchanged in every column.
- A's spare capacity is real: `cap/len` is 1.10 to 2.00 on the ownership path.
  For a single 64 KiB scalar the returned buffer holds 2N.
- The first candidate (canonicalizer change only, before the constructor
  reorder) raised the
  complete-constructor peak for `retained_large_payload/1blocks_65536B` from
  197176 to 262332 bytes (3.01x to 4.00x N): A's 2N capacity outlived the
  65 KiB block-receipt string and the `Arc` copy. Converting to the exact-size
  `Arc` before building receipts drops the slack first; the peak is 196818 at
  `8a1fb166`, and no cell exceeds its baseline peak. No `shrink_to_fit` or
  accounting change is involved; retained accounting is unchanged because the
  `Arc` payload was already exactly N.

### Served-order and cache frequencies

Identical for A and B and across all processes.

| Workload | Served | Canonical order | Noncanonical order | Cache hits | Cache misses |
|---|---|---|---|---|---|
| `transform_cold/100msgs_2KiB_mixed` | 102 | 100 | 2 | 0 | 102 |
| `transform_warm/100msgs_2KiB_mixed` | 102 | 100 | 2 | 102 | 0 |
| `transform_cold/1000msgs_2KiB_mixed` | 1002 | 1000 | 2 | 0 | 1002 |
| `transform_warm/1000msgs_2KiB_mixed` | 1002 | 1000 | 2 | 1002 | 0 |

Every cold served message is a miss (100 or 1000 canonical-order retained
originals plus 2 unordered synthetic shells); every warm served message
returned a cache entry on lookup. The hit counter is a proxy: it does not
distinguish a positive entry from `Some(None)`, so warm positive hits and
skipped construction are not observed here. Completed-output page replay is
not constructed by this driver.

### Timing (A → B, ten independent process pairs)

Per-pair ratios divide B's p50 by A's p50 within the same pair. "Pairs B<A"
counts pairs where B's p50 is lower. Per-sample setup runs outside both clocks.

| Boundary | Population | A p50 ns: median (min..max) | B p50 ns: median (min..max) | Pairs B<A | B/A p50 per pair: median (min..max) | A p95 | B p95 | A CPU ns/sample | B CPU ns/sample |
|---|---|---|---|---|---|---|---|---|---|
| canonicalizer | `retained_ascii/1blocks` | 1268 (1262..1289) | 1157 (1145..1208) | 10/10 | 0.912 (0.894..0.957) | 1321 | 1303 | 1737 | 1651 |
| full_constructor | `retained_ascii/1blocks` | 3739 (3722..3822) | 3496 (3458..3530) | 10/10 | 0.934 (0.916..0.946) | 3875 | 3581 | 4253 | 3991 |
| canonicalizer | `retained_escaped/1blocks` | 2077 (2055..2121) | 1842 (1816..1872) | 10/10 | 0.883 (0.857..0.908) | 2143 | 1908 | 2536 | 2324 |
| full_constructor | `retained_escaped/1blocks` | 4668 (4645..4766) | 4351 (4317..4407) | 10/10 | 0.930 (0.918..0.946) | 4763 | 4459 | 5174 | 4863 |
| canonicalizer | `retained_large_payload/1blocks_65536B` | 30731 (30707..30810) | 29044 (28988..29335) | 10/10 | 0.945 (0.943..0.954) | 30824 | 29093 | 31285 | 29622 |
| full_constructor | `retained_large_payload/1blocks_65536B` | 454391 (453778..454961) | 452466 (451752..453350) | 10/10 | 0.996 (0.995..0.999) | 459554 | 457795 | 456023 | 453741 |
| canonicalizer | `typed_shell/1blocks` | 949 (937..962) | 1033 (1018..1046) | 0/10 | 1.090 (1.075..1.104) | 996 | 1086 | 1410 | 1518 |
| full_constructor | `typed_shell/1blocks` | 2662 (2651..2738) | 2737 (2701..2798) | 1/10 | 1.029 (0.997..1.054) | 2716 | 2805 | 3144 | 3241 |
| canonicalizer | `one_edited_block/1blocks` | 946 (939..950) | 986 (975..1000) | 0/10 | 1.040 (1.035..1.057) | 987 | 1026 | 1399 | 1464 |
| full_constructor | `one_edited_block/1blocks` | 2637 (2624..2720) | 2696 (2667..2718) | 1/10 | 1.018 (0.993..1.030) | 2689 | 2749 | 3123 | 3221 |
| canonicalizer | `retained_ascii/65blocks` | 59580 (59038..59849) | 44826 (44725..45384) | 10/10 | 0.753 (0.747..0.763) | 61267 | 46770 | 60419 | 45746 |
| full_constructor | `retained_ascii/65blocks` | 172970 (171921..179859) | 157606 (156609..158182) | 10/10 | 0.909 (0.879..0.917) | 178554 | 163413 | 174438 | 159141 |
| canonicalizer | `retained_escaped/65blocks` | 105497 (105114..106097) | 92584 (92186..92880) | 10/10 | 0.878 (0.869..0.880) | 110345 | 96379 | 106583 | 93450 |
| full_constructor | `retained_escaped/65blocks` | 223285 (222384..232043) | 206803 (205553..207785) | 10/10 | 0.925 (0.894..0.933) | 229703 | 213112 | 225044 | 208582 |
| canonicalizer | `retained_large_payload/65blocks_65536B` | 1957471 (1932277..2014089) | 1789234 (1785445..1893290) | 10/10 | 0.917 (0.888..0.954) | 2225995 | 1892794 | 2009996 | 1811179 |
| full_constructor | `retained_large_payload/65blocks_65536B` | 32129318 (32080095..32205546) | 32022551 (31885090..32081086) | 10/10 | 0.996 (0.992..0.999) | 32294822 | 32352787 | 32145761 | 32062319 |
| canonicalizer | `typed_shell/65blocks` | 29058 (28945..29599) | 29217 (29113..29404) | 1/10 | 1.006 (0.985..1.012) | 29530 | 29509 | 29746 | 29878 |
| full_constructor | `typed_shell/65blocks` | 98877 (97960..105834) | 99707 (98698..100336) | 4/10 | 1.011 (0.946..1.024) | 99435 | 100443 | 99678 | 100527 |
| canonicalizer | `one_edited_block/65blocks` | 125941 (124871..127376) | 127039 (126179..128527) | 0/10 | 1.006 (1.001..1.026) | 131197 | 132333 | 127309 | 128368 |
| full_constructor | `one_edited_block/65blocks` | 229075 (226898..235383) | 228789 (226382..230771) | 5/10 | 0.999 (0.980..1.009) | 234944 | 234883 | 230601 | 230719 |
| transform_cold | `100msgs_2KiB_mixed` | 4198757 (4174440..4257935) | 4134314 (4043873..4199289) | 9/10 | 0.981 (0.952..1.006) | 4254088 | 4190873 | 4194809 | 4132569 |
| transform_warm | `100msgs_2KiB_mixed` | 2842097 (2823210..2882287) | 2832168 (2797459..2872604) | 7/10 | 0.994 (0.988..1.014) | 2934844 | 2943484 | 2875918 | 2869864 |
| transform_cold | `1000msgs_2KiB_mixed` | 36353054 (36083683..36614577) | 36140158 (35847882..36626145) | 9/10 | 0.995 (0.987..1.004) | 36901184 | 36692709 | 36401117 | 36180648 |
| transform_warm | `1000msgs_2KiB_mixed` | 22587402 (22301510..22913862) | 22518490 (21990840..22949250) | 5/10 | 1.001 (0.979..1.023) | 23140715 | 23004294 | 22647744 | 22594191 |

Reading, at the actual scope of each boundary:

- Canonicalizer, canonical-miss populations: B is faster in 10 of 10 pairs in
  every cell, with per-pair p50 ratios from 0.74 (`retained_ascii/65blocks`)
  to 0.95 (`retained_large_payload/1blocks`). The saving is the elided copy;
  it is proportional to N over the fixed serialization cost.
- Full constructor, canonical-miss populations: B is faster in 10 of 10 pairs;
  ratios 0.91 to 0.99. Receipt serialization and hashing dominate the large
  scalar cells, so the elided copy is a small share there.
- Unordered populations: `typed_shell/1blocks` and `one_edited_block/1blocks`
  are slower in 9 or 10 of 10 pairs, canonicalizer ratios 1.04 to 1.09 and
  full-constructor ratios 1.02 to 1.03; the 65-block unordered cells are within
  0.99 to 1.01. These cells still take the copy path and now also pay the
  aggregate order check and the earlier `Arc` conversion. This is an adverse
  effect at the scale of the reorder itself and is reported, not offset
  against the canonical cells.
- Transform cold and warm, 100 and 1000 messages: cold cells are faster in
  9 of 10 pairs with median ratios 0.98 and 0.995; warm cells have 7 and 5 of
  10 pair wins with medians 0.99 and 1.00. Every per-pair range spans or
  touches 1.0, so no transform-level change is claimed; the canonicalizer is
  a small share of a pass that also projects, renders, hashes, and builds
  receipts.
- CPU per sample moves in the same direction as the elapsed median in every
  cell except `full_constructor` / `one_edited_block/65blocks`, where elapsed
  falls 229075 to 228789 ns and CPU rises 230601 to 230719 ns.

### Host latency

Unmeasured. No real-host transform driver, including `serve_native`, is
retained, and no request-send-to-matching-terminal timing exists. No
user-visible latency claim is made.

## Failure scenario

An implementation could preserve bytes while cloning A, shrinking it, or
allocating replacement output-sized scratch. The ledger classification
requires the returned chain to grow at every step and end at the reported
capacity, and the large-scalar cells additionally forbid any allocation of at
least N bytes outside that chain. The private
`canonical_input_returns_the_serialization_buffer_and_disordered_input_does_not`
test also checks pointer, length, and capacity identity between the
post-serialization buffer and the returned buffer.

## Timing windows and dependencies

Allocation windows span `canonical_served_bytes_for_test` or
`served_message_for_test` only. The constructor peak includes A's capacity,
the `Arc` copy, block-receipt strings, and identity formatting; the early
`Arc` conversion changes their overlap, not their sizes.

## What a test must construct

Later changes to the canonicalizer or constructor rerun
`scripts/perf/canonical-output-paired-runs.sh paired <merge-base> <candidate> <out-dir>`
with the candidate's merge base as the A leg, so both legs share one harness,
and compare against this record's B column only after this record itself has
been rerun on the corrected `073f578e` harness.

## Investigation log

### Q: Did the complete-constructor peak rise?

- Sources examined: the first paired run against the canonicalizer-only
  candidate and the rerun against `8a1fb166` with both legs on one harness.
- Findings: yes for the single 64 KiB scalar until the `Arc` conversion moved
  ahead of receipt construction; afterwards every cell is at or below its
  baseline peak.
- Missing evidence: none for the declared populations.
- Conclusion: resolved with answer; the reorder is part of the landed change.

### Q: Is there a user-visible latency gain?

- Sources examined: the transform cells and the absence of a host driver.
- Findings: no detectable transform-level change at 100 or 1000 messages;
  host latency is unmeasured.
- Missing evidence: a retained real-host driver.
- Conclusion: unresolved, needs HP1's host driver; no speedup claim is made.
