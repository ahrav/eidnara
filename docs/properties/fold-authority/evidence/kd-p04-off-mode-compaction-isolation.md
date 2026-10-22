# kd-p04-off-mode-compaction-isolation

## Discovery trigger

Proposed obligation KD-P04 of #1060's Property Catalog section B (KTD2),
with the off-mode clauses of FA-E02. Ticket #1062; PR #1069. Surface: Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- Eviction scheduling returns on the latch before `due()` and its branch
  sync (`packages/pi-plugin/src/index.ts:877-880`).
- The compaction answer is `undefined` when the latch is on (`:953-956`),
  and `handBack` keeps its off-mode value `false` (`:961`).
- The tag strip runs only when the latch is off (`:985-986`).
- Witnesses in `packages/pi-plugin/src/transform/pi-delivery.test.ts`: `:257`
  (compaction-on control), `:267` (compaction-off: no eviction, unchanged
  tags, `eidnara_reduce` absent, Pi's own compaction), `:301` (a
  compaction-off process whose session runs in a folding project; fails
  without the latch return). Fold-command refusals:
  `packages/pi-plugin/src/commands/eidnara-commands.test.ts:109`, `:369`;
  `packages/pi-plugin/src/commands/eidnara-wrapup.test.ts:131`.

## Failure scenario

An additive success whose reply names a boundary would schedule
`ctx.compact` in a compaction-off process, and Pi would run a second,
native compaction with a model call.

## Timing windows and dependencies

The scheduler runs from `agent_end` on a zero-delay timer; the witnesses
wait 20 ms after each turn.

## What a test must construct

A tagged assistant; an `agent_end` after an additive success; a boot
latch that disagrees with the session's project.

## Investigation log

### Q: Is the latch return in `scheduleEviction` load-bearing?

- Sources examined: `index.ts:875-880`; `pi-delivery.test.ts:301`.
- Findings: Yes, when the boot latch is on (`compactionOff === true`,
  `index.ts:382-383`, because the boot project is unresolved) while the
  session's project folds. In every other off-mode history `due()` is
  already false.
- Missing evidence: None.
- Conclusion: resolved with answer.
