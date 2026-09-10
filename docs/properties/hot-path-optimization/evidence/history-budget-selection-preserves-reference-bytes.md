# history-budget-selection-preserves-reference-bytes

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Repeated whole-history estimation can motivate fragment caching, estimates,
or batched demotion. Exact selected bytes remain the preservation target.

## Evidence trail

- [decay_render.rs:296-338][render] computes initial tiers, joins nonempty bodies
  with `\n\n`, and demotes the first position below tier 5 until the guard stops.
- [203-240][tiers] removes tier 5 and maps tier ordinals. The fallback at
  [144-163][fallback] can repeat tier bodies, so demotion need not change text.
- [614-640][golden] compares JSON expected bodies with a no-guard estimator.
  [644-761][shape] compares actual-tokenizer costs, SHA-256, and tier counts.
  [765-800][tight] compares tight-budget JSON bodies using the real estimator.
- [token_cache.rs:165-205][cache] caches exact content counts and tests parity
  with direct tokenization, including repeated calls.

## Failure scenario

Adding fragment token counts ignores join-seam BPE effects. A cached estimate
chooses a later stopping point and drops extra history, or stops early with
different bytes. Counting tokens correctly after that selection does not repair
the changed output; both selection and final bytes must match the reference.

## Timing windows and dependencies

Input order defines age for demotion; timestamps do not authorize resorting.
Unicode, escaping, empty fragments, repeated tier bytes, and tier-5 removal
alter joined input. Fixture-local cost monotonicity is not a universal BPE law.
The optimizer may batch internal work without matching the reference's loop count.

## What a test must construct

Use the oldest-first renderer and tokenizer from commit
`913234433ae36a80a6e22c6aac14c7f9aab74386` as the fixed semantic reference.
Retain that commit's [render bodies][render-json], [tight bodies][tight-json],
[store shape][shape-json], and [store differential SHA/counts][diff-json],
including their recorded provenance. Do not regenerate expected bytes with
candidate helpers. A reference executable or test-only adapter is a packaging
choice, not a choice of semantics.
For fixed inputs and finite budgets, compare exact final bytes and actual
whole-text counts, including cache-backed/direct parity at observed boundaries.
Exercise join seams and equal-tier bodies. No byte-sum conservation or guaranteed
cache miss is assumed. Existing tests are unaudited and no comparison runs here.

## Investigation log

### Q: Which reference and goldens establish an independent stopping oracle?

- Sources examined: [The renderer][render] and [real-estimator goldens][tight].
- Findings: The fixed source commit supplies the semantic renderer/tokenizer
  and exact JSON/SHA fixtures. Reusing candidate helpers would be circular.
- Missing evidence: Reference packaging and a parity run are not supplied;
  the baseline identity and acceptance rule are no longer open decisions.
- Conclusion: Resolved as commit 913234433ae36a80a6e22c6aac14c7f9aab74386.
  Final-byte equivalence is required, not tokenization at each candidate step.

[render]: ../../../../crates/daemon/src/decay_render.rs#L296-L338
[tiers]: ../../../../crates/daemon/src/decay_render.rs#L203-L240
[golden]: ../../../../crates/daemon/src/decay_render.rs#L614-L640
[shape]: ../../../../crates/daemon/src/decay_render.rs#L644-L761
[tight]: ../../../../crates/daemon/src/decay_render.rs#L765-L800
[cache]: ../../../../crates/daemon/src/token_cache.rs#L165-L205
[fallback]: ../../../../crates/daemon/src/decay_render.rs#L144-L163
[render-json]: ../../../../crates/daemon/testdata/render-golden.json
[tight-json]: ../../../../crates/daemon/testdata/render-tight-golden.json
[shape-json]: ../../../../crates/daemon/testdata/decay-store-shape.json
[diff-json]: ../../../../crates/daemon/testdata/decay-store-differential.json
