# kd-p06-whole-array-native-submission

## Discovery trigger

Proposed obligation KD-P06 of #1060's Property Catalog section B (KTD4).
Tickets #1062 and #1063; PRs #1069 and #1070. Surface: plugin and Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- Mechanism as in [KD-E03](kd-e03-native-cold-import-prefix-omission.md).
- Pi witness `packages/pi-plugin/src/transform/pi-delivery.test.ts:318`:
  `HALF_CAP_BLOCKS` preloaded single-block rows plus the prompt submit 401
  rows, then 403 on the repeat, in compaction-off mode and for an unresolved
  project inside a compaction-on process; an arm led by Pi's compaction
  summary keeps that row first.
- OpenCode witness
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1336`.
- Negative control (run once and reverted in #1069 and #1070): supplying
  `measure` unconditionally fails both witnesses.

## Failure scenario

401 single-block rows would become a 400-row suffix, and the applied pass
would drop the oldest row.

## Timing windows and dependencies

A cold window crossing half the cap on its first pass.

## What a test must construct

No anchor; a host above half the cap; native authority by mode and by an
unresolved routed project; a leading compaction summary.

## Investigation log

### Q: Does the per-project gate (OQ-1) reach the routed case?

- Sources examined: `index.ts:584`; `pi-delivery.test.ts:318`.
- Findings: Yes; the unresolved routed arm runs in a compaction-on process
  and submits the whole array.
- Missing evidence: None.
- Conclusion: resolved with answer.
