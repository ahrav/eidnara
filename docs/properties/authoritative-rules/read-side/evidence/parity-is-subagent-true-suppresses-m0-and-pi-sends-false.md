# parity-is-subagent-true-suppresses-m0-and-pi-sends-false

## Discovery trigger

Plan KTD8 ("Pi keeps `is_subagent: false` because the extension API exposes
`parentSession` only at `newSession`, not per context event; the asymmetry
is documented"), U6 step 4 ("Document that Pi in-process children receive m0
(constant `is_subagent: false`) and OpenCode host-reported subagents do
not"), and R13's conditioning on "the same `is_subagent` value". Serves R13;
the discrepancy-log row "Pi `is_subagent` is constant false".

## Evidence trail

- `crates/daemon/src/transform.rs:11723-11733`: `if !req.is_subagent { ...
  push m0 slot ...; push m1 slot ... }`. A subagent pass serves neither
  frozen prefix unit.
- `crates/daemon/src/transform.rs:671, 973`: `is_subagent` is a request
  field decoded from the wire (`wire.is_subagent`).
- `docs/host-wire-protocol.md:966`: `mid_turn`, `is_subagent`,
  `tool_present` are optional booleans defaulting to `false`. No text
  describes the Pi asymmetry.
- `packages/pi-plugin/src/transform/pi-pass-inputs.ts:110`: `is_subagent:
  false` literal.
- `packages/opencode-plugin/src/hooks/context/transform-pass-fields.ts:66`:
  `is_subagent: args.passInputs.is_subagent === true`, sourced from the
  host's pass inputs.
- `packages/opencode-plugin/src/hooks/context/command-handler.ts:430,
  652`: OpenCode has an `isSubagentSession` predicate used to gate
  `/eidnara-wrapup`; the host knows subagent sessions.
- `packages/pi-plugin/src/index.ts:332-340`: two kinds of Pi children:
  spawned (`EIDNARA_PI_SUBAGENT=1`, no registration, so no pass at all) and
  in-process (`@gotgenes/pi-subagents`, which re-run the factory and so run
  passes with `is_subagent: false`).
- `crates/daemon/tests/*`: `grep -rn is_subagent crates/daemon/tests` finds
  only fixture JSON; no test pins the m0 suppression.

## Failure scenario

Legal witness: an OpenCode subagent session's pass carries `is_subagent:
true`; the response recipe inserts no m0 and no m1 row; no rule reaches the
subagent. A Pi in-process child's pass carries `false`; it receives m0 with
the rules. The wire document's `is_subagent` row says so.

Smallest violating traces:
1. Suppression lost: a refactor of the serving loop emits the frozen m0 unit
   regardless of `is_subagent`; OpenCode subagents receive project rules the
   host asked to withhold, and the parity statement's domain silently widens.
2. Pi derives `is_subagent` incorrectly: a future change sets it `true` for
   all sessions in a process that has ever hosted a child (process-global
   latch misuse); top-level Pi sessions lose rules.
3. Undocumented asymmetry: the parity e2e compares a Pi in-process child
   with an OpenCode subagent and fails; without the documented asymmetry the
   failure is read as a daemon defect and "fixed" by serving m0 to
   subagents.
4. Parity test ignores the flag: AE8's test runs Pi and OpenCode with
   different `is_subagent` values and asserts equality; it fails for the
   wrong reason or passes only because both happen to be top-level.

## Timing windows and dependencies

- None; the gate is per request.
- Bounds the domain of `parity-identical-request-fields-yield-identical-
  rule-sets`.

## What a test must construct

1. Daemon unit or integration test: two passes identical except
   `is_subagent`; the `true` pass's served slots contain no payload with
   `<project-memory>` (and no m1 placeholder); the `false` pass's do.
2. Pi field test: every body built by `piPassFields` has `is_subagent ===
   false` (pin the constant so a change is deliberate).
3. OpenCode field test: `is_subagent` is `true` iff the host pass input says
   so.
4. Documentation check: `docs/host-wire-protocol.md` 7.10.3's `is_subagent`
   row (or an adjacent note) names the Pi constant and the OpenCode source;
   a doc test or a grep in CI can pin the sentence.
5. Parity e2e: both passes use `is_subagent: false`; a second case with
   OpenCode `true` asserts the OpenCode block is absent and is not compared.

## Investigation log

### Q: Can Pi derive `is_subagent` per pass?
- Sources examined: plan KTD8; `index.ts:338-340`.
- Findings: the plan states the extension API exposes `parentSession` only
  at `newSession`. A per-session map populated at `newSession` and read in
  the `context` handler by session id would carry the flag; the plan chose
  to document rather than build this. Not verified against the Pi API in
  this pass.
- Missing evidence: the Pi `newSession` event payload and whether the
  session id is stable across the two events.
- Conclusion: needs human input - accept the asymmetry (plan position) or
  schedule the per-session derivation; either way this record's check holds.

### Q: Is any existing test pinning the m0 suppression under `is_subagent`?
- Sources examined: `crates/daemon/tests/transform_canonical_memory.rs`;
  `grep -rn is_subagent crates/daemon/tests`.
- Findings: none; in-crate tests reference `is_subagent` for cache TTL,
  emergency sampling, reasoning, render, and terse compression
  (`transform.rs:12595, 17235, 19282, 23710, 26990`), not m0 serving.
- Conclusion: resolved with answer - no existing check; the record's step 1
  is new.
