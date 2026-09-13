# referenceable-json-rejects-hooks-before-reading

## Discovery trigger

The parent specification's TE19 requires source validation without invoking
accessors, proxies or serialization hooks. Recursive field snapshots must not
invoke those hooks while attempting to detect mutation.

This evidence covers the source-guard precursor only. The parent companion
catalog is unavailable in this checkout. No claim about its full acceptance
status is inferred from this supplement.

## Evidence trail

Source base: `352ce13fdac3024485e7d05c1679fe0db74c8d98` plus this precursor's
uncommitted changes. References use paths and symbols because added files do
not exist at the source base.

- `packages/opencode-plugin/src/hooks/context/transform-capture.ts`:
  `ReferenceableWalk` validates descriptors before visiting their values.
  `captureMessages` retains membership and field tapes; rechecks compare exact
  primitive values, named array metadata, key order, descriptor attributes,
  container terminators and entry counts. Root metadata has its own tape.
- `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts`:
  `run` captures before message reads and directory preflight. The existing
  pass checks also recheck the source. The pre-apply check repeats immediately
  before replacement. Prefix reuse compares captured fields directly.
- `packages/opencode-plugin/src/hooks/context/module-wire.ts`:
  `resolveOrdinalsForModule` invokes the caller's source guard after asynchronous
  scanning and before its synchronous message filtering and projection.
- `packages/opencode-plugin/src/hooks/context/hook.ts`:
  root, index, `info` and session ID reads use own-data descriptors. Capture
  precedes hook-side directory resolution, with a recheck after that await.
- `packages/opencode-plugin/src/plugin/messages-transform.ts`:
  pre-hook source validation protects array copying. A shared cheap root check
  protects promise assimilation before and after the hook. The post-hook check
  cannot suppress publication already made through the host array. Existing
  rollback behavior is outside this precursor's ownership claim.

## Failure scenario

A getter installed on `parts` during an ordinal scan can execute before a
post-resolver guard runs. The resolver therefore checks before its synchronous
source-reading half. A proxy root can trap even during length or membership
inspection; the non-trapping runtime proxy predicate must run first.

## Timing windows and dependencies

The direct tests distinguish initial rejection from mutation after an earlier
valid dispatch. They pause directory and transport promises, and schedule
mutation from the ordinal provider before the asynchronous scan resumes.
Each of the four pause windows has an unmutated control that must publish the
independent expected output and ACK its own delivery. Mutated cases require no
additional transform send and no ACK. A separate retry-check test schedules
mutation after the synchronous check but before its await resumes, then proves
that the known delivery receives one NACK. Pure helpers also test content edits,
membership replacement, removal, metadata renames and descriptor changes.

## What a test must construct

- Construct an unsupported root, member or nested value and count every getter
  and proxy trap independently of dispatch counts.
- Demonstrate a successful readonly-input transform with exact approved output
  and preserved destination and payload identity.
- Capture supported input, reach the named await, then install the hook.
- Preserve optional undefined object fields and harmless hidden data as positive
  controls; change hidden data and require the recheck to fail.
- Exercise cumulative walk exhaustion and acyclic reference amplification.

## Investigation log

### Q: What has executable evidence?

- Sources examined: the source and test paths above, and the donor's guard walk.
- Findings: the focused six-file Bun run passes 193 tests and 1,441 assertions;
  plugin typechecking passes on Node 24.18.0. Existing module-wire and wrapper
  tests and the serialized-frame tests run alongside the added witnesses.
  A separate Node 24.18.0 runtime probe rejects normal and revoked proxies,
  detects root metadata changes, reads hidden array length, and rejects an
  installed accessor with zero traps.
  These counts describe the recorded run, not a claim that every test proves
  TE19.
- Missing evidence: PR2's complete ownership, admission and publication fault
  matrix, native-addon-enabled CI coverage, and U5 measurements. Supplied review
  findings and repairs are recorded in [the dispositions](../review-dispositions.md).
- Conclusion: partial source-guard evidence is present. The parent contract
  remains unresolved until the deferred work and controller gates complete.

Focused commands run from `packages/opencode-plugin`, with Node 24.18.0 first
on PATH:

```sh
bun test src/hooks/context/transform-capture.test.ts \
  src/hooks/context/rust-mode-transform.test.ts src/hooks/context/hook.test.ts \
  src/plugin/messages-transform.test.ts src/hooks/context/module-wire.test.ts \
  src/hooks/context/module-wire-frame.test.ts
bun run typecheck
```

Exact-file Biome checks, the repository comment-marker scan and `git diff
--check` also pass. The unchanged sequential benchmark script passes a
1,000-message, one-sample smoke run. That run checks its oracle, not performance
acceptance or a comparison with the historical baseline.

`bun run check:repo` passes with Node 24.18.0 first on PATH, including root
typechecking, lint, tests, builds and the generated-TUI drift check. The full
OpenCode package suite reports 3,351 passing tests. Lint reports warnings in
unchanged files; the ten-file scoped check reports none. The native package
tests report `addon_unavailable`, so this root gate is not evidence of an
enabled native addon. `bun run --cwd packages/opencode-plugin smoke` passes
both smart-note Wasm bundle checks and TUI import checks.
