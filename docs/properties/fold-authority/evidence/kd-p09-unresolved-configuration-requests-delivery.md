# kd-p09-unresolved-configuration-requests-delivery

## Discovery trigger

Proposed obligation KD-P09 of #1060's Property Catalog section B (KTD8).
Ticket #1062; PR #1069. Surface: Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- An unresolved admission withdraws fold authority in the Pi loader
  (`packages/pi-plugin/src/config/index.ts:480-515`), so the session's
  project resolves compaction off.
- Witnesses: `packages/pi-plugin/src/transform/pi-context-handler.test.ts:247`
  (routed, asserts `transforms.length > 0` and `outcome === "compacted"`);
  `packages/pi-plugin/src/transform/pi-delivery.test.ts:367` (boot cwd,
  three passes with `tool_present: false`, Pi's compaction proceeds);
  `:318` (routed, whole array inside a compaction-on process).

## Failure scenario

A malformed project file that silenced delivery would hide memory from
every session in that project.

## Timing windows and dependencies

None. An existing session with applied `eidnara` authority keeps it until a
quiescent bind (FA-N03, FA-N04); OQ-3 defers that history.

## What a test must construct

A malformed project file at the boot cwd and in a routed project.

## Investigation log

### Q: Does the boot-cwd case need a different latch?

- Sources examined: `index.ts:382-384`; `pi-delivery.test.ts:367`.
- Findings: No. A malformed boot project latches compaction off, and the
  pipeline still runs.
- Missing evidence: None.
- Conclusion: resolved with answer.
