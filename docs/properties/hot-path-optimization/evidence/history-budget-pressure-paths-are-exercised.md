# history-budget-pressure-paths-are-exercised

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Final fit does not establish repeated guard pressure. A meaningful preservation
campaign needs input that challenges more than one reference demotion.

## Evidence trail

- [decay_render.rs:321-338][loop] evaluates joined body cost and demotes the
  oldest remaining nonarchived position.
- [509-526][characters] uses a character-count fixture and checks fit plus
  newest survival; it does not certify production-tokenizer step count.
- [765-800][tight] compares real-estimator golden bytes, but its `fired` counter
  increments on final fit or empty output, not independent initial pressure.
- [644-761][shape] supplies real-budget differential cases. Their existence
  alone does not carry a multi-demotion witness into this catalog.

## Failure scenario

The candidate is always compared on histories that already fit or need one
step. A batched selection error after the first demotion remains undetected.
A marker based on the candidate's loop count can disappear after a correct
batching optimization even when the intended pressure still exists.

## Timing windows and dependencies

The reference must use fixed initial tiers, ordered input, and the actual
estimator. Its initial body and body after one legal demotion both exceed the
positive budget, with another demotable tier left. Repeated equal-tier bytes are
valid; neither step must increase or decrease token count monotonically.

## What a test must construct

Record those two reference costs and remaining demotability before candidate
execution. Use this slug as a constant `sometimes` marker, and compare candidate
final bytes separately under H1. Do not require candidate loop iterations or
cache misses. Existing checks are unaudited and no new witness runs here.

## Investigation log

### Q: Which fixture certifies more than one reference demotion?

- Sources examined: [The guard][loop], [tight goldens][tight], and
  [store-shape comparisons][shape].
- Findings: The final-fit counter is compatible with zero guard work, so it
  cannot serve as this record's independent pressure marker.
- Missing evidence: Initial and first-demotion production-tokenizer costs for
  chosen fixtures are not recorded in this task.
- Conclusion: Fixture certification is unresolved. It must be performed by the
  test-design handoff, not inferred from a test name or a historical run.

[loop]: ../../../../crates/daemon/src/decay_render.rs#L321-L338
[characters]: ../../../../crates/daemon/src/decay_render.rs#L509-L526
[tight]: ../../../../crates/daemon/src/decay_render.rs#L765-L800
[shape]: ../../../../crates/daemon/src/decay_render.rs#L644-L761
