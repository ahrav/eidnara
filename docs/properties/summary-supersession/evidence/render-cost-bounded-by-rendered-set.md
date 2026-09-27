# render-cost-bounded-by-rendered-set

## Discovery trigger

Specification [#835](https://github.com/ahrav/eidnara/issues/835), Property
Catalog record `render-cost-bounded-by-rendered-set` (bound,
default-production), derived at `265df096`, and constraint C-1: supersession
adds no store read, and its only work is one pass over the claims of the loaded
set R, at most 8 x |R| entries and 8 x |R| substring searches. The M3 ticket
[#840](https://github.com/ahrav/eidnara/issues/840) sets the criteria: a
committed bound test at 10^6 messages under both correction regimes, render
time beyond the segment load flat in H (largest H within 10% of smallest H, 30
samples per point, median and p99), and row size at the cap against the spec
estimate.

## Evidence trail

`crates/daemon/src/test_support/synthetic_history.rs::SyntheticHistory` seeds
H rows of `span` messages each; `ClaimRegime` selects `None`,
`EveryThirdMessage` (one correction per 3 messages, at most 8 per row), or
`HalfPercent` (one per 200). Keys come from 64 values, so later claims
supersede earlier ones.

`crates/daemon/src/m0_compose.rs::bounded_read_tests::the_claims_pass_adds_no_store_work_and_visits_at_most_eight_claims_per_loaded_row`
runs H = 100, 4,000 (span 2) and 50,000 (span 20, so N = 10^6). For each
regime it records the statement-work ledger (SQL, rows, VM steps) for m0 at
budgets 20, 60,000, and 10,000,000 and for m1 with 1 and 200 new rows. It
asserts both claim regimes produce ledgers equal to `ClaimRegime::None`, that
claims over the loaded rows are at most 8 x rows at each budget, and that the
dense regime loads a non-zero claim count.

Commit `1c30058c` adds a test-support counter,
`crates/daemon/src/decay_render.rs::CLAIMS_VISITED`, that `live_claims`
bumps once per claim; it compiles only under `test` or the `test-support`
feature. The bound test now asserts that an m0 compose visits exactly the
claims held by R (one scan) and at most `CLAIMS_PER_SEGMENT x |R|`, that an
m1 compose visits exactly twice the claims on its rows (one scan for
corrections, one for the block), that both regimes put claims in R at every
scale point, and that corrections change the rendered m0 bytes.

The committed test does not check these spec clauses directly:

- At most 8 x |R| substring searches per compose. This holds because each
  row has at most 8 corrections and `apply_corrections` does one `find` per
  correction.
- At most 8 markers or footer entries per rendered segment, for the same
  reason.
- About 1.9 KB added per segment before the guard. A marker is at most a
  64-byte key, a 128-byte value, and about 20 bytes of syntax, so 8
  markers stay under 1.7 KB. This is not measured.
- At most 8 x |R| entries in the m1 block, one entry per claim on its rows.

`crates/daemon/src/decay_render.rs::render_rows` runs `corrections_for` once
per compose over R. `crates/daemon/src/m1_compose.rs::compose_m1` and
`render_memory_updates` work only over the rows `load_history_segments_above`
returns.

`crates/daemon/src/decay_render.rs::render_decayed_history_segments` holds the
budget guard: while the estimate of the joined body exceeds the budget, it
demotes the oldest row below tier 5 by one tier, re-renders that row,
re-joins the whole body, and re-estimates it, for up to 5 x |R| iterations.

## Failure scenario

A per-claim store query, or a liveness scan over rows outside R, would make
each compose grow with H or N. Separately, a render step that is super-linear
in R makes a HARD pass slow under dense corrections even when the claims pass
itself is bounded.

## Timing windows and dependencies

No interleaving. The cost depends on #826's bounded read, which keeps |R|
near 2,489 at budget 60,000 regardless of H, and on the budget guard, which
footers push into many iterations.

## What a test must construct

A 10^6-message session under both regimes and none; the statement-work ledger
compared across regimes; claims visited compared to 8 x |R|. For time, a
driver that times `compose_m0` and subtracts a separately timed fold read.

## Investigation log

### Q: Is the structural bound held at N = 10^6?

- Sources examined: the bound test above, run at HEAD `c38af85a` with
  `cargo +1.98 nextest run -p daemon -p memory-store --all-features --locked`
  filtered to this part's tests.
- Findings: the test passed (17 of 17 in the run, 33.4 s for this test).
  Store work is equal with and without claims, and visits stay within 8 x |R|.
  At `5bdeaf6b` the test also counts visits through `CLAIMS_VISITED`, so
  the one-scan bound for m0 and the two-scan bound for m1 are asserted, not
  inferred from claims held.
- Missing evidence: direct checks of the substring-search, per-segment
  marker, per-segment size, and m1-entry clauses listed above.
- Conclusion: resolved with answer for the store-work and visit bounds; the
  other clauses hold by source reading.

### Q: Is render time beyond the load flat in H?

- Sources examined: an uncommitted driver at HEAD `c38af85a`, rustc 1.98.0,
  Linux 6.12.103 x86_64, 192 CPUs, release build, budget 60,000, 30 samples
  per point after 5 warm-ups. Times are microseconds of `compose_m0` minus a
  separately timed fold read.
- Findings: at span 20 with N = 50k, 200k, 1M (H = 2,500, 10,000, 50,000;
  |R| = 2,487 to 2,489), medians are: no claims 2,990 / 3,091 / 3,080 (p99
  3,100 / 3,236 / 3,452); one correction per 200 messages 3,212 / 3,322 /
  3,283 (p99 3,293 / 3,401 / 3,427); one per 3 messages 657,812 / 717,669 /
  720,020 (p99 661,737 / 720,769 / 733,718). The largest H is within 10% of
  the smallest in all three regimes (+3.0%, +2.2%, +9.5%). At N = 10^6 with
  span 400 / 100 / 20: no claims 3,067 / 3,082 / 3,091; half-percent 6,422 /
  4,025 / 3,284 (claims in R 4,968 / 1,241 / 246); every-third 944,196 /
  964,130 / 724,627 (claims in R 19,872 / 19,872 / 16,560). With N fixed,
  claims per row fall as H grows, so time falls; no point grows with H.
  Two further interleaved runs of the dense fixed-span row at `c38af85a`
  gave medians 656,395 / 717,079 / 720,249 and 658,540 / 719,436 / 722,266
  at H = 2,500 / 10,000 / 50,000 (largest over smallest +9.7% and +9.7%,
  against +9.5% in the first run). Nearly all of the step lies between
  H = 2,500 and 10,000, where two more legacy rows of the generator
  (distances 2,600 and 2,900) join R (|R| = 2,487 versus 2,489); 10,000 to
  50,000 adds +0.3%.
  Measurement caveats: "p99" at n = 30 is the sample maximum. "Beyond the
  load" is the per-iteration difference of `compose_m0` and a separately
  timed fold read, which includes compose-side work other than the fold
  read; the load always precedes compose, so compose's internal read is
  warm. Flatness is judged on the fixed-span sweep, which holds claims in R
  constant. The fixed-N = 10^6 sweep confounds H with claims per row and
  fails a symmetric plus-or-minus 10% reading, but shows no increase with
  H.
  Provenance: the driver's sha256 is
  `3c1218dfe76d5a2ab92df23b1fbf56fe595f3563176c408c30c15736930d5617`. The
  timing ran at `c38af85a` plus the uncommitted driver module. `c38af85a`
  to `5bdeaf6b` adds only docs, test code, the test-support counter, and
  the store changes of `0231f2f4`; none is on the timed path except the
  counter, which compiles only with test support.
- Missing evidence: the driver and raw report are not committed, by the #840
  artifact decision, so a reader cannot rerun them from the repository.
- Conclusion: resolved with answer for flatness on the fixed-span sweep;
  confidence medium because the timing is not a committed check.

### Q: Why does the dense regime cost about 0.7 s?

- Sources examined: a profile of the dense case (H = 10,000, span 100,
  |R| = 2,489, 19,808 corrections) and
  `crates/daemon/src/decay_render.rs::render_decayed_history_segments`.
- Findings: rendering without the budget guard takes 3.4 ms and 124,590
  tokens; with the guard, 961.8 ms and 59,970 tokens. Without claims it takes
  1.2 ms unguarded and 5.7 ms guarded. Footers raise rendered bytes above the
  curve's target, so the guard runs many iterations, and each re-joins and
  re-estimates the whole body, which is quadratic in |R|. The claims pass
  stays within 8 x |R|. The dense regime spends 0.66 to 0.96 s beyond the
  load, 200 to 300 times the no-claims baseline, from the guard's full
  re-estimate per demotion in `render_decayed_history_segments`. The cost
  is flat in H. It is a known regression that needs a follow-up ticket.
- Missing evidence: a decision on whether the guard should count footer bytes
  or estimate incrementally.
- Conclusion: needs human input. The record's structural bound holds; the
  guard's quadratic cost under dense corrections is an open design question.

### Q: How large is a row at typical claim counts and at the cap?

- Sources examined: the uncommitted driver over the 388-row store-shape
  fixture with synthesized claims.
- Findings: 3 typical claims per tiered row store 106 bytes per claim and 319
  bytes per segment in the `claims` cell. 8 claims at the maximum bounds
  store 441 bytes per claim and 3,526 bytes per segment, against the spec
  estimate of about 450 bytes and 3.6 KB.
- Missing evidence: none beyond the uncommitted driver.
- Conclusion: resolved with answer; the cap matches the estimate.
