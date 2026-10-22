# kd-p01-every-mode-submission

## Discovery trigger

Proposed obligation KD-P01 of #1060's Property Catalog section B (KTD1).
Ticket #1062; PR #1069 (head `6d527babf`). Surface: Pi.

## Evidence trail

All references are verified at `07ea883b5`, the head of #1071, which holds
#1068 to #1071.

- The transform is built and the handler registered unconditionally
  (`packages/pi-plugin/src/index.ts:565-566`). One return precedes
  `piTransform.run` (`:578`): a compaction-off event for a session with no
  admitted project identity (`!eidnaraFolds && !deps.projectIdentity`,
  `:572`), added by #1069 for the home directory without
  `allow_home_project`; every other event with a session id reaches the
  pipeline.
- Witnesses: `packages/pi-plugin/src/transform/pi-delivery.test.ts:184` (three
  passes for three events under an empty chain and under
  `compaction.enabled: false`), `:217` (zero transforms for a home-directory
  session without `allow_home_project` under both compaction-off modes),
  `:422` (an unresolved boot project),
  `packages/pi-plugin/src/transform/pi-context-handler.test.ts:247` (an
  unresolved routed project), and
  `packages/pi-plugin/src/index-env-guard.test.ts:54` (the entry registers
  `context` with no chain).

## Failure scenario

An off-mode event for a session with an admitted project identity that
returned before the pipeline delivered none of m0's blocks.

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
