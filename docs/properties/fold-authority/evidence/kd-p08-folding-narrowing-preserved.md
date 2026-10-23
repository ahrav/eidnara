# kd-p08-folding-narrowing-preserved

## Discovery trigger

Proposed obligation KD-P08 of #1060's Property Catalog section B (KTD4).
Tickets #1062 and #1063; PRs #1069 and #1070. Surface: plugin and Pi.

## Evidence trail

All references are verified at `07ea883b5`, the head of #1071, which holds
#1068 to #1071.

- While Eidnara folds, both adapters keep the hook; `coldHead` selects the
  suffix and pins its head
  (`packages/opencode-plugin/src/hooks/context/transform-session-client.ts:860-896`).
- Pi witness: the folding arm of
  `packages/pi-plugin/src/transform/pi-delivery.test.ts:341` sends
  `HALF_CAP_BLOCKS` rows, and the repeat keeps the same head.
- OpenCode witness:
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1444`
  sends `host.slice(1)`, and the repeat keeps `host[1]` with
  `HALF_CAP_BLOCKS + 2` rows.
- Existing Pi cold-import cases
  (`packages/pi-plugin/src/transform/pi-transform.test.ts:471`) still pass
  with `eidnaraFolds: true`.

## Failure scenario

If folding also omitted the hook, a cold folding session would upload its
whole history and fold it at once.

## Timing windows and dependencies

A repeat after the pin and before the first anchor.

## What a test must construct

The same oversized input under a chain.

## Investigation log

### Q: Are the folding bytes unchanged?

- Sources examined: `pi-transform.ts:268-275`; `opencode-transform-adapter.ts:397`.
- Findings: Yes; under folding each adapter builds the same closure as
  before.
- Missing evidence: None.
- Conclusion: resolved with answer.
