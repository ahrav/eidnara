# kd-p05-on-mode-machinery-preservation

## Discovery trigger

Proposed obligation KD-P05 of #1060's Property Catalog section B (KTD2),
a regression obligation. Ticket #1062; PR #1069. Surface: Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- #1069 edits no case of `packages/pi-plugin/src/transform/pi-eviction.test.ts`
  and only the unresolved-configuration case of
  `packages/pi-plugin/src/transform/pi-context-handler.test.ts` (`:247`).
- The `bun run check:repo` run at `6d527babf` passed every case of both
  files (pi-plugin 421 passed).
- On-mode guards: the eviction scheduler's `due()` and refusal backoff
  (`packages/pi-plugin/src/index.ts:875-918`), the compaction answer and
  `handBack` (`:946-976`).

## Failure scenario

A changed guard would evict without a boundary or cancel an overflow
compaction.

## Timing windows and dependencies

None.

## What a test must construct

The existing folding and eviction fixtures.

## Investigation log

### Q: Did any folding case change its expectation?

- Sources examined: `git diff f04c54e4d..6d527babf -- packages/pi-plugin/src/transform/pi-context-handler.test.ts packages/pi-plugin/src/transform/pi-eviction.test.ts`.
- Findings: Only the unresolved-configuration case changed, from zero
  transforms to `transforms.length > 0`, as KTD8 requires.
- Missing evidence: None.
- Conclusion: resolved with answer.
