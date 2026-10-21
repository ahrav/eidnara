# kd-p01-every-mode-submission

## Discovery trigger

Proposed obligation KD-P01 of #1060's Property Catalog section B (KTD1).
Ticket #1062; PR #1069 (head `6d527babf`). Surface: Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- The transform is built and the handler registered unconditionally
  (`packages/pi-plugin/src/index.ts:565-566`); the per-project early return
  is gone, and every event with a session id reaches `piTransform.run`
  (`:576`).
- Witnesses: `packages/pi-plugin/src/transform/pi-delivery.test.ts:182` (three
  passes for three events under an empty chain and under
  `compaction.enabled: false`), `:367` (an unresolved boot project),
  `packages/pi-plugin/src/transform/pi-context-handler.test.ts:247` (an
  unresolved routed project), and
  `packages/pi-plugin/src/index-env-guard.test.ts:54` (the entry registers
  `context` with no chain).

## Failure scenario

An off-mode event that returned before the pipeline delivered none of m0's
blocks.

## Timing windows and dependencies

An admission refusal is a decline inside the pipeline (KD-P07).

## What a test must construct

The full runtime in each mode, and an admission refusal.

## Investigation log

### Q: Does the subagent entry still register no handler?

- Sources examined: `index-env-guard.test.ts:38`.
- Findings: Yes; the environment guard returns before any registration.
- Missing evidence: None.
- Conclusion: resolved with answer.
