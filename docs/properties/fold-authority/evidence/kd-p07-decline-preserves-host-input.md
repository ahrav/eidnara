# kd-p07-decline-preserves-host-input

## Discovery trigger

Proposed obligation KD-P07 of #1060's Property Catalog section B (KTD4),
amended for OpenCode by OQ-2 option (a). Tickets #1062 and #1063; PRs #1069
and #1070. Surface: plugin and Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- Pi sets `failOpen: false`
  (`packages/pi-plugin/src/transform/pi-transform.ts:300`); a declined pass
  returns no replacement and Pi keeps its own array. A held admission lease
  declines before the client (`:234`).
- OpenCode falls back only over a retained basis
  (`packages/opencode-plugin/src/hooks/context/transform-session-client.ts:1168-1170`);
  the invocation refusal declines at `:1707`.
- Pi witnesses: `packages/pi-plugin/src/transform/pi-delivery.test.ts:394`
  (`invalid_params`, `queue_full`, an invocation refusal; first pass and
  after a success) and `:427` (admission refusal; first pass and repeat).
- OpenCode witnesses:
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1375`
  (invocation refusal leaves every host value in place) and `:1410` (after a
  prior success, `queue_full` publishes `[m0, m1, ...grown]`).

## Failure scenario

A Pi refusal that published the last applied output would serve stale m0
over a changed array.

## Timing windows and dependencies

A decline after a prior successful pass.

## What a test must construct

Daemon errors; a known context limit with a growing candidate; a held
admission lease.

## Investigation log

### Q: Does the OpenCode fallback omit a host message?

- Sources examined: `rust-mode-window.test.ts:1410-1436`.
- Findings: No. The publication is the retained m0 and m1 followed by every
  host message of the grown array.
- Missing evidence: None.
- Conclusion: resolved with answer (OQ-2 option a, approved 2026-10-21).
