# kd-e03-native-cold-import-prefix-omission

## Discovery trigger

Existing-behavior record KD-E03 of #1060's Property Catalog section A
(`kd-e-native-cold-import-prefix-omission`), a baseline defect
characterization. Tickets #1062 and #1063; PRs #1069 (head `6d527babf`) and
#1070 (head `dbdac1150`). Surface: plugin and Pi.

Status: invalidated by both adapters.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- `coldHead` reads a surviving pin first, then calls `source.measure?.()`;
  with no hook it returns `undefined`, and the client submits the whole array
  (`packages/opencode-plugin/src/hooks/context/transform-session-client.ts:851-860`).
- Pi supplies `measure` only while the session's project lets Eidnara fold:
  `eidnaraFolds: isCompactionEnabled(deps.config)`
  (`packages/pi-plugin/src/index.ts:584`) and the conditional spread
  (`packages/pi-plugin/src/transform/pi-transform.ts:268-275`).
- OpenCode supplies it only when `compactionOff` is not `true`
  (`packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts:397`);
  the hook resolves `compactionOff` once
  (`packages/opencode-plugin/src/hooks/context/hook.ts:227`).
- Pi witness: `packages/pi-plugin/src/transform/pi-delivery.test.ts:318`.
  OpenCode witness:
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1336`,
  which asserts each host message is the published value at `index + 2` by
  identity.

## Failure scenario

At baseline an OpenCode session in compaction-off mode with no anchor and a
host above half the cap submitted the newest suffix, and publication
replaced the whole host array with m0, m1, and that suffix. Nothing
summarized the prefix, so the provider lost it.

## Timing windows and dependencies

The pin is consulted before the hook; withholding the hook leaves a pin
from a folding run in place. That history needs a project switch, which
OQ-3 defers.

## What a test must construct

No anchor; no pin; a host above 400 CK blocks; an accepted additive reply.

## Investigation log

### Q: Which evidence invalidates KD-E03 end to end?

- Sources examined: the two adapter cases; PR #1071's description.
- Findings: The adapter harnesses. #1071 adds no oversized end-to-end arm;
  its delivery tests run below half the cap.
- Missing evidence: None for the invalidation.
- Conclusion: resolved with answer.
