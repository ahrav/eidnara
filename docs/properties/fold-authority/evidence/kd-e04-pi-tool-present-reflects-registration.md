# kd-e04-pi-tool-present-reflects-registration

## Discovery trigger

Existing-behavior record KD-E04 of #1060's Property Catalog section A
(`kd-e-pi-tool-present-reflects-registration`). Ticket #1062; PR #1069 (head
`6d527babf`, `bun run check:repo` exit 0). Surface: Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- Registration: `setEidnaraReduceRegisteredGlobally(!compactionOff)`
  (`packages/pi-plugin/src/index.ts:384`); the tool registry omits
  `eidnara_reduce` when `compactionOff`
  (`packages/pi-plugin/src/tools/index.ts:96`).
- Pass fields: `reduceRegistered: !compactionOff` and
  `todowriteRegistered: todowriteEnabled` (`index.ts:595-596`), mapped to
  `tool_present` and `todo_tool_present` by `piPassFields`
  (`packages/pi-plugin/src/transform/pi-pass-inputs.ts`).
- At baseline the field was hardcoded `true`.
- Witnesses: the capability matrix
  (`packages/pi-plugin/src/transform/pi-pass-inputs.test.ts:100`) over all
  four pairs; the integrated compaction-off passes
  (`packages/pi-plugin/src/transform/pi-delivery.test.ts:182`) with
  `tool_present: false` and `todo_tool_present: true`; the cross-binding
  comparison (`:213`) with `[true, true]` in a compaction-on process and
  `[false, true]` in a compaction-off one; the registration set
  (`packages/pi-plugin/src/tools/index.test.ts:19`).

## Failure scenario

A `true` for an unregistered tool lets the daemon activate a tagging
surface whose tag prefixes the off-mode strip leaves in assistant text.

## Timing windows and dependencies

Registration is latched at boot; a routed project cannot change it.

## What a test must construct

A compaction-on pass; an off-mode field-builder case; an integrated
off-mode pass after registration.

## Investigation log

### Q: Is the field derived from registration rather than configuration?

- Sources examined: `index.ts:382-384`, `:595`.
- Findings: Yes. A withdrawn routed project inside a compaction-on process
  still reports `true`, because the process registered the tool.
- Missing evidence: None.
- Conclusion: resolved with answer.
