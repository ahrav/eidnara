# pi-context-handler-is-gated-by-compaction-on

## Discovery trigger

Lens: architecture (S5) and claimed guarantees (P3). The brief named "Pi
createPiTransform registration tied to compactionOff". KTD8 and OQ4 propose
registering the handler whenever the project binds, so the current gating
is the behavior U6 changes and the parity asymmetry the plan's discrepancy
log records ("Pi delivers no project memory with compaction off").

## Evidence trail

- `packages/pi-plugin/src/index.ts:90` `PI_TRANSFORM_AVAILABLE = true`.
- `:381-384`: comment "Pi registers tools once per process, so compaction
  registration does not follow later /cd config changes";
  `compactionRequested = isCompactionEnabled(config)`; `compactionOff =
  !PI_TRANSFORM_AVAILABLE || !compactionRequested`;
  `setEidnaraReduceRegisteredGlobally(!compactionOff)`.
- `:565` `const piTransform = compactionOff ? undefined :
  createPiTransform({ moduleClient, branchOf });`
- `:566-609`: `if (piTransform) { pi.on("context", async (event, ctx) => {
  ... if (!isCompactionEnabled(deps.config)) return undefined; ... await
  piTransform.run({...}) ... }); info("registered context transform
  handler"); }`. The per-event gate at `:573` reads the project resolved for
  `ctx.cwd` (`resolveCurrentProjectDeps`).
- `:150-157` `handlePiSessionBeforeCompact` returns early under
  `compactionOff` so native Pi compaction proceeds; the same flag feeds the
  `/eidnara-flush`, `/eidnara-recomp`, `/eidnara-wrapup` deps (`:404-407`).
- Daemon side: the additive-only path composes m0 with the block
  (`additive-only-pass-still-composes-project-memory`), so the daemon would
  serve the block if asked; Pi never asks.
- OpenCode contrast: `packages/opencode-plugin/src/hooks/context/command-handler.ts:511-519`
  treats `compactionOff` as a command-availability flag; its transform hook
  registration is outside this pass's files.
- Tests: `index.test.ts:70-75` covers `handlePiSessionBeforeCompact`;
  `eidnara-commands.test.ts:114`, `:374` and `tools/index.test.ts:37` pass
  `compactionOff: true` to commands and tools; no test records which
  `pi.on` events the factory registers under either flag value.
- Recent history: `fe2c98930`, `416aa7374`, `7d89fe739` adjust Pi's
  compaction handoff around the same flag.

## Failure scenario

Today: a Pi user with `compaction.enabled: false` never sends a transform
request, so no `<project-memory>` reaches the model; rules recorded through
the plan's channels would be invisible on that harness while OpenCode serves
them. After U6: if only the boot gate at `:565` is lifted and the per-event
gate at `:573` stays, a `/cd` into a compaction-off project silently drops
rules again; if the per-event gate is lifted and the boot gate stays, a
compaction-off boot still registers nothing.

## Timing windows and dependencies

`compactionOff` is fixed at boot (`:381`). The per-event gate re-resolves
the project per `context` event. `isCompactionEnabled` reads the merged
config (`packages/pi-plugin/src/config`).

## What a test must construct

1. A recording `ExtensionAPI` stub (the `start-notice-order.test.ts:28`
   shape with `on` captured); run the default export with a config fixture
   whose `compaction.enabled` is `false`; assert no `context` registration
   and no `"registered context transform handler"` log line.
2. Same with `true`; assert exactly one `context` registration; invoke it
   with a `ctx` whose resolved project config disables compaction; assert it
   returns `undefined` and the module client recorded no `transform` call.
3. Coverage: `pi_factory_ran_with_compaction_off` fires in case 1.

## Investigation log

### Q: Should the per-event gate survive U6 as a fold-only gate?

- Sources examined: `index.ts:573`; plan KTD8 ("Pi registers its context
  handler whenever the project binds, with compaction gating only the
  fold"); `createPiTransform` (`packages/pi-plugin/src/transform/pi-transform.ts`)
  not read in full.
- Findings: the plan names one gate ("the fold"); the code has two
  (registration and per-event). Where the fold-only gate lives after U6 is
  unstated.
- Missing evidence: the implementation decision.
- Conclusion: needs human input.
