# u1-copy-elision-paired-measurement

## Discovery trigger

Plan U1 returns the serialization buffer A when every object is already in
canonical order, and plan U4 requires the baseline and candidate to run side by
side on the frozen ten-pair schedule before any saving is claimed. This record
compares the U0 baseline harness revision `c1dafa76` (A) against candidate
`0827d6f00cd5e277214cdcfdfe52534a35e42eb8` (B), which contains the
canonicalizer change and the constructor's early `Arc` conversion.

## Evidence trail

- Raw artifacts: [`runs/u1-paired-c1dafa76-vs-0827d6f0/`](runs/u1-paired-c1dafa76-vs-0827d6f0/):
  `pair-01-A.json` .. `pair-10-B.json` and
  [`provenance.json`](runs/u1-paired-c1dafa76-vs-0827d6f0/provenance.json).
- Schedule: `scripts/perf/canonical-output-paired-runs.sh paired c1dafa76 0827d6f0 <dir>`;
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
- The first candidate (canonicalizer change only, before the constructor
  reorder) raised the
  complete-constructor peak for `retained_large_payload/1blocks_65536B` from
  197176 to 262332 bytes (3.01x to 4.00x N): A's 2N capacity outlived the
  65 KiB block-receipt string and the `Arc` copy. Converting to the exact-size
  `Arc` before building receipts drops the slack first; the peak is 196818 at
  `0827d6f0`, and no cell exceeds its baseline peak. No `shrink_to_fit` or
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
| canonicalizer | `retained_ascii/1blocks` | 1267 (1244..1278) | 1138 (1125..1176) | 10/10 | 0.903 (0.891..0.922) | 1307 | 1276 | 1743 | 1647 |
| full_constructor | `retained_ascii/1blocks` | 3721 (3697..3841) | 3434 (3418..3474) | 10/10 | 0.923 (0.890..0.937) | 3852 | 3535 | 4208 | 3940 |
| canonicalizer | `retained_escaped/1blocks` | 2038 (2018..2080) | 1771 (1745..1808) | 10/10 | 0.869 (0.847..0.879) | 2109 | 1855 | 2504 | 2239 |
| full_constructor | `retained_escaped/1blocks` | 4586 (4542..4763) | 4321 (4287..4358) | 10/10 | 0.943 (0.900..0.949) | 4703 | 4425 | 5093 | 4808 |
| canonicalizer | `retained_large_payload/1blocks_65536B` | 30640 (30622..30728) | 29059 (29031..29070) | 10/10 | 0.948 (0.946..0.949) | 30765 | 29108 | 31227 | 29644 |
| full_constructor | `retained_large_payload/1blocks_65536B` | 454111 (453760..454400) | 452179 (451884..452521) | 10/10 | 0.996 (0.995..0.997) | 459680 | 457698 | 455718 | 453496 |
| canonicalizer | `typed_shell/1blocks` | 927 (920..946) | 980 (964..987) | 0/10 | 1.054 (1.028..1.068) | 975 | 1033 | 1383 | 1436 |
| full_constructor | `typed_shell/1blocks` | 2579 (2571..2685) | 2659 (2646..2674) | 1/10 | 1.028 (0.992..1.036) | 2637 | 2721 | 3064 | 3156 |
| canonicalizer | `one_edited_block/1blocks` | 923 (917..933) | 945 (938..949) | 0/10 | 1.025 (1.005..1.035) | 966 | 993 | 1394 | 1403 |
| full_constructor | `one_edited_block/1blocks` | 2564 (2554..2652) | 2601 (2576..2622) | 1/10 | 1.013 (0.981..1.023) | 2614 | 2651 | 3047 | 3078 |
| canonicalizer | `retained_ascii/65blocks` | 59406 (59045..60997) | 43913 (43597..44564) | 10/10 | 0.740 (0.717..0.750) | 62850 | 44968 | 60393 | 44658 |
| full_constructor | `retained_ascii/65blocks` | 171842 (170782..178340) | 156149 (155247..156432) | 10/10 | 0.908 (0.877..0.913) | 177588 | 161577 | 173409 | 157635 |
| canonicalizer | `retained_escaped/65blocks` | 104451 (103653..107036) | 89484 (89203..89835) | 10/10 | 0.857 (0.834..0.863) | 109132 | 93062 | 105534 | 90224 |
| full_constructor | `retained_escaped/65blocks` | 220586 (218968..231183) | 203315 (202205..204143) | 10/10 | 0.923 (0.879..0.928) | 227472 | 208798 | 222451 | 204856 |
| canonicalizer | `retained_large_payload/65blocks_65536B` | 1931566 (1927126..1952367) | 1787812 (1784739..1793856) | 10/10 | 0.925 (0.916..0.931) | 1953444 | 1803914 | 1936130 | 1790233 |
| full_constructor | `retained_large_payload/65blocks_65536B` | 32055604 (32010386..32129667) | 31924584 (31867060..32035619) | 9/10 | 0.996 (0.993..1.001) | 32137673 | 31988560 | 32063252 | 31934474 |
| canonicalizer | `typed_shell/65blocks` | 28915 (28836..29783) | 28891 (28723..28999) | 6/10 | 0.999 (0.971..1.005) | 29295 | 29173 | 29583 | 29487 |
| full_constructor | `typed_shell/65blocks` | 98502 (97921..105063) | 98734 (98609..99601) | 1/10 | 1.003 (0.939..1.017) | 98981 | 99196 | 99285 | 99472 |
| canonicalizer | `one_edited_block/65blocks` | 123742 (122657..124893) | 123765 (122893..124725) | 6/10 | 0.999 (0.984..1.014) | 128768 | 128704 | 125500 | 125022 |
| full_constructor | `one_edited_block/65blocks` | 225834 (224610..233411) | 225763 (224253..227427) | 7/10 | 0.998 (0.974..1.006) | 231883 | 231394 | 227583 | 227163 |
| transform_cold | `100msgs_2KiB_mixed` | 4199205 (4131502..4311008) | 4100439 (4038437..4130405) | 10/10 | 0.969 (0.951..0.995) | 4256494 | 4187894 | 4188759 | 4100599 |
| transform_warm | `100msgs_2KiB_mixed` | 2823450 (2790580..2884609) | 2833252 (2790939..2887946) | 2/10 | 1.006 (0.980..1.024) | 2999334 | 3034013 | 2863529 | 2871813 |
| transform_cold | `1000msgs_2KiB_mixed` | 36080522 (35765881..36757640) | 35884871 (35497840..36210364) | 7/10 | 0.992 (0.977..1.012) | 36575051 | 36319468 | 36116664 | 35919120 |
| transform_warm | `1000msgs_2KiB_mixed` | 22443970 (21822378..22967230) | 22391551 (21982927..23146880) | 4/10 | 1.002 (0.957..1.031) | 23016327 | 22971675 | 22442311 | 22390838 |

Reading, at the actual scope of each boundary:

- Canonicalizer, canonical-miss populations: B is faster in 10 of 10 pairs in
  every cell, with per-pair p50 ratios from 0.74 (`retained_ascii/65blocks`)
  to 0.95 (`retained_large_payload/1blocks`). The saving is the elided copy;
  it is proportional to N over the fixed serialization cost.
- Full constructor, canonical-miss populations: B is faster in 10 of 10 pairs;
  ratios 0.91 to 0.99. Receipt serialization and hashing dominate the large
  scalar cells, so the elided copy is a small share there.
- Unordered populations: `typed_shell/1blocks` and `one_edited_block/1blocks`
  are slower in 9 or 10 of 10 pairs, canonicalizer ratios 1.03 to 1.05 and
  full-constructor ratios 1.01 to 1.03; the 65-block unordered cells are within
  0.97 to 1.02 with mixed pair wins. These cells still take the copy path and
  now also pay the aggregate order check and the earlier `Arc` conversion. This
  is an adverse effect at the scale of the reorder itself and is reported, not
  offset against the canonical cells.
- Transform cold and warm, 100 and 1000 messages: the 100-message cold cell
  is faster in 10 of 10 pairs with a median ratio of 0.97; the other three
  cells have pair wins of 2, 7, and 4 of 10 with median ratios 0.99 to 1.01
  and per-pair ranges spanning 1.0. Only the smallest cold cell shows a
  consistent transform-level change; the canonicalizer is a small share of a
  pass that also projects, renders, hashes, and builds receipts.
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

- Sources examined: the first paired run against the canonicalizer-only
  candidate and the rerun against `0827d6f0`.
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
