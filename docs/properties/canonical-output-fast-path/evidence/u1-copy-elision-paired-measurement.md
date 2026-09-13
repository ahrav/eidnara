# u1-copy-elision-paired-measurement

## Discovery trigger

Plan U1 returns the serialization buffer A when every object is already in
canonical order, and plan U4 requires the baseline and candidate to run side by
side on the frozen ten-pair schedule before any saving is claimed. This record
compares the U0 baseline harness revision `c1dafa76` (A) against candidate
`2050f3a63aaa903504c63a05920f0758c0f68a51` (B), which contains the
canonicalizer change and the constructor's early `Arc` conversion.

## Evidence trail

- Raw artifacts: [`runs/u1-paired-c1dafa76-vs-2050f3a6/`](runs/u1-paired-c1dafa76-vs-2050f3a6/):
  `pair-01-A.json` .. `pair-10-B.json` and
  [`provenance.json`](runs/u1-paired-c1dafa76-vs-2050f3a6/provenance.json).
- Schedule: `scripts/perf/canonical-output-paired-runs.sh paired c1dafa76 2050f3a6 <dir>`;
  odd pairs run A then B, even pairs run B then A, every run a fresh release
  process built from a detached worktree of its own revision.
- Baseline record: [u0-baseline-measurement](u0-baseline-measurement.md).
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
  requested bytes and peak fall by N. Typed and one-edited-block populations
  are unchanged in every column.
- A's spare capacity is real: `cap/len` is 1.10 to 2.00 on the ownership path.
  For a single 64 KiB scalar the returned buffer holds 2N.
- The first candidate (`7378f53d`, canonicalizer change only) raised the
  complete-constructor peak for `retained_large_payload/1blocks_65536B` from
  197176 to 262332 bytes (3.01x to 4.00x N): A's 2N capacity outlived the
  65 KiB block-receipt string and the `Arc` copy. Converting to the exact-size
  `Arc` before building receipts drops the slack first; the peak is 196818 at
  `2050f3a6`, and no cell exceeds its baseline peak. No `shrink_to_fit` or
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
originals plus 2 unordered synthetic shells); every warm served message is a
positive hit and constructs nothing. Completed-output page replay is not
constructed by this driver.

### Timing (A → B, ten independent process pairs)

Per-pair ratios divide B's p50 by A's p50 within the same pair. "Pairs B<A"
counts pairs where B's p50 is lower. Per-sample setup runs outside both clocks.

| Boundary | Population | A p50 ns: median (min..max) | B p50 ns: median (min..max) | Pairs B<A | B/A p50 per pair: median (min..max) | A p95 | B p95 | A CPU ns/sample | B CPU ns/sample |
|---|---|---|---|---|---|---|---|---|---|
| canonicalizer | `retained_ascii/1blocks` | 1256 (1241..1277) | 1133 (1119..1153) | 10/10 | 0.901 (0.892..0.913) | 1302 | 1268 | 1726 | 1618 |
| full_constructor | `retained_ascii/1blocks` | 3714 (3704..3737) | 3464 (3450..3488) | 10/10 | 0.933 (0.926..0.941) | 3845 | 3567 | 4200 | 3973 |
| canonicalizer | `retained_escaped/1blocks` | 2042 (2016..2050) | 1795 (1765..1825) | 10/10 | 0.881 (0.864..0.894) | 2115 | 1879 | 2508 | 2265 |
| full_constructor | `retained_escaped/1blocks` | 4592 (4578..4612) | 4331 (4294..4379) | 10/10 | 0.944 (0.937..0.954) | 4707 | 4444 | 5088 | 4876 |
| canonicalizer | `retained_large_payload/1blocks_65536B` | 30643 (30606..30680) | 29043 (29020..29162) | 10/10 | 0.948 (0.946..0.952) | 30782 | 29083 | 31227 | 29626 |
| full_constructor | `retained_large_payload/1blocks_65536B` | 454021 (453727..454338) | 452084 (451738..452300) | 10/10 | 0.996 (0.995..0.996) | 459360 | 457521 | 455561 | 453415 |
| canonicalizer | `typed_shell/1blocks` | 924 (919..945) | 966 (952..999) | 0/10 | 1.044 (1.020..1.084) | 976 | 1018 | 1392 | 1427 |
| full_constructor | `typed_shell/1blocks` | 2573 (2561..2594) | 2687 (2660..2706) | 0/10 | 1.042 (1.031..1.054) | 2635 | 2748 | 3057 | 3161 |
| canonicalizer | `one_edited_block/1blocks` | 924 (915..929) | 940 (929..948) | 0/10 | 1.017 (1.003..1.034) | 965 | 985 | 1382 | 1393 |
| full_constructor | `one_edited_block/1blocks` | 2557 (2547..2641) | 2643 (2621..2674) | 1/10 | 1.035 (0.997..1.050) | 2615 | 2699 | 3067 | 3121 |
| canonicalizer | `retained_ascii/65blocks` | 59518 (58678..59953) | 44080 (43885..44727) | 10/10 | 0.742 (0.735..0.751) | 62772 | 47503 | 60543 | 45508 |
| full_constructor | `retained_ascii/65blocks` | 171843 (171228..173309) | 156021 (155001..157161) | 10/10 | 0.907 (0.894..0.915) | 177976 | 161942 | 173582 | 157645 |
| canonicalizer | `retained_escaped/65blocks` | 104145 (103802..105131) | 89640 (88931..90268) | 10/10 | 0.859 (0.850..0.863) | 109522 | 93074 | 105318 | 90462 |
| full_constructor | `retained_escaped/65blocks` | 220774 (219528..221682) | 203381 (202318..204339) | 10/10 | 0.922 (0.913..0.926) | 227412 | 209729 | 222659 | 205209 |
| canonicalizer | `retained_large_payload/65blocks_65536B` | 1933363 (1927123..1941021) | 1786901 (1785028..1789817) | 10/10 | 0.924 (0.922..0.929) | 1953569 | 1801822 | 1936991 | 1788846 |
| full_constructor | `retained_large_payload/65blocks_65536B` | 32065786 (32015294..32119255) | 31915623 (31871527..31971187) | 10/10 | 0.995 (0.993..0.998) | 32143620 | 31978396 | 32073038 | 31921347 |
| canonicalizer | `typed_shell/65blocks` | 29004 (28840..29160) | 28800 (28672..28894) | 10/10 | 0.994 (0.988..0.998) | 29304 | 29088 | 29657 | 29406 |
| full_constructor | `typed_shell/65blocks` | 98031 (97909..98607) | 99161 (98761..99744) | 0/10 | 1.011 (1.004..1.018) | 98720 | 99725 | 98841 | 99971 |
| canonicalizer | `one_edited_block/65blocks` | 124494 (123620..125803) | 123929 (122949..124978) | 7/10 | 0.995 (0.984..1.002) | 130015 | 129570 | 125841 | 125247 |
| full_constructor | `one_edited_block/65blocks` | 226122 (224816..228163) | 225476 (223451..226705) | 6/10 | 0.999 (0.979..1.003) | 232579 | 231864 | 227692 | 227157 |
| transform_cold | `100msgs_2KiB_mixed` | 4189928 (4108629..4301122) | 4091865 (4056640..4214548) | 7/10 | 0.974 (0.945..1.010) | 4290089 | 4173525 | 4193355 | 4101366 |
| transform_warm | `100msgs_2KiB_mixed` | 2827778 (2803763..2891012) | 2827688 (2802437..2896016) | 4/10 | 1.001 (0.977..1.016) | 2911059 | 2914232 | 2859334 | 2864959 |
| transform_cold | `1000msgs_2KiB_mixed` | 36129588 (35457760..36814433) | 36080938 (35593903..36504817) | 7/10 | 0.994 (0.980..1.018) | 36745169 | 36479248 | 36162713 | 36065605 |
| transform_warm | `1000msgs_2KiB_mixed` | 22354367 (21931501..23009647) | 22470194 (22129995..22799909) | 4/10 | 1.009 (0.969..1.026) | 23004762 | 23177695 | 22421693 | 22486579 |

Reading, at the actual scope of each boundary:

- Canonicalizer, canonical-miss populations: B is faster in 10 of 10 pairs in
  every cell, with per-pair p50 ratios from 0.74 (`retained_ascii/65blocks`)
  to 0.95 (`retained_large_payload/1blocks`). The saving is the elided copy;
  it is proportional to N over the fixed serialization cost.
- Full constructor, canonical-miss populations: B is faster in 10 of 10 pairs;
  ratios 0.91 to 0.99. Receipt serialization and hashing dominate the large
  scalar cells, so the elided copy is a small share there.
- Unordered populations: canonicalizer ratios 0.99 to 1.03 and full-constructor
  ratios 0.99 to 1.02. `typed_shell/65blocks` full constructor is slower in
  10 of 10 pairs by about 1.1%; `typed_shell/1blocks` canonicalizer by about
  3%. These cells still take the copy path and now also pay the aggregate
  order check and the earlier `Arc` conversion. This is an adverse effect at
  the scale of the reorder itself and is reported, not offset against the
  canonical cells.
- Transform cold and warm, 100 and 1000 messages: pair wins are 7/10, 4/10,
  7/10, 4/10 with median ratios 0.97 to 1.01 and per-pair ranges spanning 1.0.
  No transform-level change is detectable at these sizes; the canonicalizer
  is a small share of a pass that also projects, renders, hashes, and builds
  receipts for a thousand messages.
- CPU per sample moves with the elapsed medians in every cell.

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
`scripts/perf/canonical-output-paired-runs.sh paired c1dafa76 <candidate>` and
compare against this record's B column, keeping the baseline binary in the
comparison.

## Investigation log

### Q: Did the complete-constructor peak rise?

- Sources examined: the first paired run against `7378f53d` and the rerun
  against `2050f3a6`.
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
