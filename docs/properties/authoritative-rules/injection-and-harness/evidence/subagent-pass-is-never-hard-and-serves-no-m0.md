# subagent-pass-is-never-hard-and-serves-no-m0

## Discovery trigger

Lens: architecture; existing tests (quiet area). The brief named
"is_subagent gating of m0". The plan's parity requirement R13 is stated "for
the same `is_subagent` value" and KTD8 fixes Pi at `false`, so the daemon's
treatment of the flag is the mechanism that decides which sessions receive
rules.

## Evidence trail

- Request field: `crates/daemon/src/transform.rs:671` `pub is_subagent:
  bool`; wire `docs/host-wire-protocol.md:966` "`mid_turn`, `is_subagent`,
  `tool_present` | optional, default `false`".
- Plan gate: `transform.rs:4216-4222`
  `if req.is_subagent { plan = if Defer { PassPlan::Defer } else {
  PassPlan::Soft } } else if lineage_state.force_hard { plan = Hard }`.
- Serve gate: `transform.rs:11723-11733` pushes the `m0` and `m1` frozen
  units into the output only when `!req.is_subagent`.
- Early subagent step: `transform.rs:4455-4470` runs a `Soft` core step for
  subagents outside Defer, with no m0 composition.
- Other subagent gates in the same file (`:3627` auto search, `:3830` terse
  compression, `:5190` tail reclaim, `:2075` cache TTL) are owned by the
  rendering catalog and not re-cataloged.
- Fold-off path: `apply_additive_only` (`transform.rs:2834-3200`) tests
  `is_subagent` only for the cache TTL (`:2943`) and the scheduler's
  `BoundaryBypass` (`:2959`); the plan comes from `classify` with no
  subagent override, and the served output always begins with the frozen
  `m0` and `m1` units (`:3178-3196`). A subagent request on this path
  receives the block.
- OpenCode sets the flag from `isSubagentSession`
  (`packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts:458, 507`),
  tested at `hook.test.ts:1790-1803`. Pi sends `false`
  (`packages/pi-plugin/src/transform/pi-pass-inputs.ts:110`).
- Tests naming subagents in `transform.rs` (`:12595`, `:17235`, `:19282`,
  `:23710`, `:26990`) assert TTL, emergency latch, reasoning, tagging, and
  terse compression; none asserts the block's absence.

## Failure scenario

A regression that dropped the `!req.is_subagent` guard at `:11723` would
serve a parent's frozen m0 (history, docs, project memory) into every
OpenCode child prompt, which the subagent entry's own doc forbids
(`packages/pi-plugin/src/subagent-entry.ts:11-14` for Pi's spawned children:
"The full entry injects key files, project documentation, user profiles, and
session history into subagent prompts" is the thing it avoids). Under the
plan, rules would reach subagents on one harness and not the other.

## Timing windows and dependencies

Per request. A session whose flag changes between passes (OpenCode restores a
child as a subagent session, `hook.test.ts:1796-1799`) serves m0 only on the
passes that carry `false`.

## What a test must construct

1. On the `canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot`
   fixture (`transform.rs:13851`), set `request.is_subagent = true`; assert
   `action != "HARD"` and the served bytes contain neither
   `<project-memory>` nor `<session-history>`; flip the flag back and assert
   the block appears.
2. The same with `ctx.fold_authority.eidnara_folds = false`; assert the
   served messages begin with the frozen m0 (the recorded asymmetry).
3. Coverage: `transform_subagent_request_with_injectable_rows` fires.

## Investigation log

### Q: Does the additive-only path serve the block to a subagent request?

- Sources examined: `transform.rs:2834-3200` (`apply_additive_only`),
  `:3341` (entry condition), `:2943`, `:2959` (the only `is_subagent` reads
  in that range).
- Findings: no subagent gate on the plan or on the output; `:3178-3196`
  pushes `m0` and `m1` unconditionally, then the request messages.
- Missing evidence: a witness test (item 2 above) and the design intent.
- Conclusion: resolved for the code (the block is served); the intent needs
  human input, recorded on the catalog record.
