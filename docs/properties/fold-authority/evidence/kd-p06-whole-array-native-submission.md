# kd-p06-whole-array-native-submission

## Discovery trigger

Proposed obligation KD-P06 of #1060's Property Catalog section B (KTD4).
Tickets #1062 and #1063; PRs #1069 and #1070. Surface: plugin and Pi.

## Evidence trail

All references are verified at `07ea883b5`, the head of #1071, which holds
#1068 to #1071.

- Mechanism as in [KD-E03](kd-e03-native-cold-import-prefix-omission.md).
- Pi witness `packages/pi-plugin/src/transform/pi-delivery.test.ts:341`:
  `HALF_CAP_BLOCKS` preloaded single-block rows plus the prompt submit 401
  rows, then 403 on the repeat, in compaction-off mode and for an unresolved
  project inside a compaction-on process; an arm led by Pi's compaction
  summary keeps that row first.
- OpenCode witness
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1417`.
- Negative control (run once and reverted in #1069 and #1070): supplying
  `measure` unconditionally fails both witnesses.
- Stored authority still native under a folding configuration: the adapter
  supplies the hook and the first cold pass dispatches a half-cap suffix;
  the daemon's additive response reports `history_summarizer.no_fire:
  "native_authority"`, the client records it and declines the pass as
  `native_authority` with its pin cleared
  (`packages/opencode-plugin/src/hooks/context/transform-session-client.ts:1669-1680`),
  and every later `null` pass sends the whole array while that state holds
  (`:1361`). Witnesses:
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1342`
  (decline, then the whole array), `:1389` (the pinned suffix returns once
  a response reports another authority), and
  `packages/pi-plugin/src/transform/pi-delivery.test.ts:390`.
- A pin from a folding run does not survive a withdrawn configuration:
  `coldHead` clears it when `measure` is absent (`:863-866`);
  `packages/pi-plugin/src/transform/pi-transform.test.ts:505` and
  `packages/opencode-plugin/src/hooks/context/transform-session-client.test.ts:894`.

## Failure scenario

401 single-block rows would become a 400-row suffix, and the applied pass
would drop the oldest row.

## Timing windows and dependencies

A cold window crossing half the cap on its first pass; a bind whose
authority change is still pending when that pass dispatches, which costs
one declined pass before the whole array is sent.

## What a test must construct

No anchor; a host above half the cap; native authority by mode and by an
unresolved routed project; a leading compaction summary; for the transition
arm, a daemon that answers a cold suffix with `no_fire: "native_authority"`.

## Investigation log

### Q: Does the per-project gate (OQ-1) reach the routed case?

- Sources examined: `index.ts:571`, `:586`; `pi-delivery.test.ts:341`.
- Findings: Yes; the unresolved routed arm runs in a compaction-on process
  and submits the whole array.
- Missing evidence: None.
- Conclusion: resolved with answer.
