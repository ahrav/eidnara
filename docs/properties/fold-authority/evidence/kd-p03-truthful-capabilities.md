# kd-p03-truthful-capabilities

## Discovery trigger

Proposed obligation KD-P03 of #1060's Property Catalog section B (KTD3).
Ticket #1062; PR #1069. Surface: Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- `tool_present` is `!compactionOff` (`packages/pi-plugin/src/index.ts:595`),
  the latch that registers `eidnara_reduce` (`:384`).
- Witnesses as in [KD-E04](kd-e04-pi-tool-present-reflects-registration.md):
  `packages/pi-plugin/src/transform/pi-pass-inputs.test.ts:100`;
  `packages/pi-plugin/src/transform/pi-delivery.test.ts:182`, `:213`.
- Negative control (run once and reverted in #1069): hardcoding
  `reduceRegistered: true` fails the capability tests.

## Failure scenario

An unregistered reduce tool reported `true` requests a tagging surface.

## Timing windows and dependencies

None.

## What a test must construct

Both modes; todowrite enabled and disabled.

## Investigation log

### Q: Does `todo_tool_present` keep its semantics?

- Sources examined: `index.ts:596`; the capability matrix.
- Findings: Yes; it reports `todowriteEnabled` in both modes.
- Missing evidence: None.
- Conclusion: resolved with answer.
