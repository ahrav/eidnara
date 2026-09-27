# live-claim-is-max-seq-per-key-in-rendered-set

## Discovery trigger

The #835 Property Catalog record of the same name, derived by the
invariant-modeling pass at `265df096`, and D-6: among the claims of the
loaded set R, `live(key)` is the claim with the maximum `(sequence, idx)`,
and a pure `corrections_for(segments)` runs once per compose, outside the
tier computation and the pressure-retry loop. The M2 ticket #839 names this
record, its required faults (equal-sequence claims with different `idx`,
three segments sharing a key, a key present once), and the acceptance
criterion that `corrections_for` returns equal results across the four
pressure multipliers of one compose.

## Evidence trail

`crates/daemon/src/decay_render.rs::live_claims` walks the segments in slice
order and each segment's claims in position order, and replaces a key's entry
only when the stored `(sequence, idx)` compares less than the new one. The
comparison is explicit on the tuple, so the result does not depend on the
slice being sorted. The map is a `BTreeMap`.

`crates/daemon/src/decay_render.rs::corrections_for` maps each segment to the
claims whose `(sequence, idx)` differs from their key's live position, each
carrying the claim's own anchor and key with the live claim's value and
ordinal. Output order is segment order, then claim order.

`crates/daemon/src/decay_render.rs::render_rows` zips each segment with its
`corrections_for` entry and stores it on the render row. The function takes
no tier, pressure, budget, or importance input.

`crates/daemon/src/m0_compose.rs::compose_m0` calls `render_rows` once on the
fold rows, then passes the rows to `render_m0_with_decay_pressure_retry`. That
function's `render` closure forwards `inputs.history_segments` unchanged at
each of up to four multipliers (1.0 and three retries of x1.15), so every
retry sees the same correction vectors.

`crates/daemon/src/m1_compose.rs::compose_m1` calls `render_rows` on the rows
above the folded sequence, and `render_memory_updates` calls the same
`live_claims` over the same rows.

Tests:

- `crates/daemon/src/decay_render.rs::tests::correction_properties::corrections_match_the_naive_argmax`
  generates up to six rows of up to four claims over three keys and compares
  `corrections_for` with `reference_corrections`, which scans all claims per
  claim and takes `max_by_key` on `(sequence, idx)`. It also asserts that the
  claim count equals the correction count plus the distinct key count, which
  is the "live or exactly one correction" partition.
- `crates/daemon/src/decay_render.rs::tests::corrections_for_keeps_the_latest_claim_live_per_key`
  covers three rows sharing `k.v`, a key present once (`k.once`), and two
  claims of `k.w` in one row, and asserts `corrections_for(&[])` is empty.
- `crates/daemon/src/m0_compose.rs::correction_compose_tests::a_hard_under_budget_pressure_renders_one_correction_set_and_replays`
  composes under a 500-token budget that demotes S1, and asserts the footer
  carries the same live value that the roomy compose splices.

## Failure scenario

A liveness pass keyed on `sequence` alone lets the earlier of two same-row
claims win, so a row's own later statement renders as superseded. A pass
recomputed inside the retry loop over the rows a tier keeps drops a corrector
at tier 5 and serves the stale value as current.

## Timing windows and dependencies

None inside one compose: the input is the loaded rows, read in one snapshot.
The retry loop re-renders up to four times, and the corrections are fixed
before it starts. Store-wide meaning depends on R being a newest suffix,
which is the separate record `superseding-claim-is-rendered-whenever-stale-claim-is`.

## What a test must construct

Rows with two claims of one key in one row (equal sequence, different
`idx`), three rows sharing a key, and a key present once; an independent
argmax reference; a compose under a budget that forces demotion so the retry
loop runs.

## Investigation log

### Q: Does any code path compute liveness per pressure multiplier or per tier?

- Sources examined: `m0_compose.rs::compose_m0`,
  `m0_compose.rs::render_m0_with_decay_pressure_retry`,
  `decay_render.rs::render_one_history_segment`.
- Findings: `corrections_for` has two callers, `render_rows` and the
  `corrections_render_to_fixed_bytes` test. `render_one_history_segment`
  reads `c.corrections` and never recomputes them.
- Missing evidence: no test counts `corrections_for` calls per compose; the
  once-per-compose claim rests on reading the call graph.
- Conclusion: resolved with answer; liveness is computed once in
  `render_rows`, before the retry loop.

### Q: Do the named checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon --all-features --locked --lib -E 'test(decay_render::) | test(correction) | test(every_loaded_claim)'`
  at `c38af85a`.
- Findings: 28 tests run, 28 passed, including both property tests and the
  pressure-replay compose test.
- Missing evidence: none.
- Conclusion: resolved with answer.
