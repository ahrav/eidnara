# parity-pi-context-handler-registers-whenever-project-binds

## Discovery trigger

Plan requirement R13 ("Pi registers its context handler whenever the project
binds, with compaction gating only the fold"), KTD8 ("Pi splits
`createPiTransform` registration from `compactionOff` ... and the daemon
serves PASSTHROUGH plus m0 when the fold is off"), OQ4 (blocking before U6),
U6 test scenario "Pi with compaction off receives `<project-memory>` on a
top-level session". Serves R13, AE8.

## Evidence trail

- `packages/pi-plugin/src/index.ts:381-384`: `compactionRequested =
  isCompactionEnabled(config)`; `compactionOff = !PI_TRANSFORM_AVAILABLE ||
  !compactionRequested`; `setEidnaraReduceRegisteredGlobally(!compactionOff)`.
  The comment at `:381` says Pi registers tools once per process.
- `packages/pi-plugin/src/index.ts:565`: `const piTransform = compactionOff ?
  undefined : createPiTransform(...)`; `:566-607` the `context` handler is
  registered only inside `if (piTransform)`; `:572` the handler returns
  `undefined` when `!isCompactionEnabled(deps.config)`.
- `packages/pi-plugin/src/index.ts:593`: the handler passes
  `reduceRegistered: true` unconditionally into `piPassFields`, which
  becomes `tool_present` (`pi-pass-inputs.ts:111`).
- `crates/daemon/src/transform.rs:3341-3342`: when `!authority.eidnara_folds`
  (and not a subagent lineage switch) the pass takes `apply_additive_only`.
- `crates/daemon/src/transform.rs:3052`: the additive path composes m0 with
  `compose_additive_m0` on `PassPlan::Hard | MigrateHard`, which renders
  `render_memory_block(ctx.project_memory_rows(), "project-memory")`
  (`transform.rs:2826-2830`).
- `crates/daemon/src/transform.rs:11723-11728`: the frozen `m0` unit is
  served as a synthetic slot when `!req.is_subagent`.
- `docs/host-wire-protocol.md:995`: "A daemon with compaction disabled
  never resets: it serves the pass additively and keeps the session's
  history" (7.10.4).
- `packages/opencode-plugin/src/hooks/context/command-handler.ts:512-520`:
  OpenCode with `compactionOff` still runs its transform hook (only the
  flush, recomp, and wrapup commands are gated), so OpenCode already
  receives additive m0 with compaction off; Pi does not. This is the
  asymmetry the record removes.

## Failure scenario

Legal witness: Pi config `compaction.enabled: false`; the project binds; the
`context` handler runs on the first turn; the daemon answers an additive
`Hard` pass whose recipe inserts an m0 row carrying `<project-memory>` with
the seeded rule; later passes serve the frozen m0.

Smallest violating traces:
1. Handler not registered: `createPiTransform` stays behind `compactionOff`;
   no pass runs; no rules reach a compaction-off Pi user; OpenCode in the
   same configuration gets them. Parity fails for the configuration class.
2. Handler registered but early return kept: the handler runs and returns at
   `!isCompactionEnabled(deps.config)`; same outcome as 1 with the
   registration log line claiming otherwise.
3. Tool presence lie: the handler sends `tool_present: true` while the
   reduce tool is not registered (`setEidnaraReduceRegisteredGlobally(false)`);
   the daemon's CC-U1 and nudge logic (`transform.rs:3039` `cc_u1_active`
   uses `req.tool_present`) treats a missing tool as present. Not a rules
   defect, but a regression the split can introduce.
4. Fold authority mismatch: Pi stops gating but the daemon's fold authority
   for the route still folds (because the daemon's own config says
   compaction on); Pi then receives folded output it did not expect. The
   plan says the daemon serves PASSTHROUGH plus m0; the test must assert
   the pass class is additive (`PASSTHROUGH` or `HARD` on the additive path),
   not a fold.

## Timing windows and dependencies

- Pi registers once per process; a `/cd` to a project with different
  compaction config does not re-register (`index.ts:381`). The record's
  "whenever the project binds" is per process start under the plan's
  current Pi constraints; a test must not assume re-registration on `/cd`.
- Depends on the daemon additive path staying m0-bearing (verified at HEAD).

## What a test must construct

1. Pi plugin unit test: with `compaction.enabled: false` and
   `PI_TRANSFORM_AVAILABLE`, `pi.on("context", ...)` is registered (spy on
   the extension API) and the handler does not return early for a bound
   project.
2. Field test: the pass body's `tool_present` equals the actual reduce-tool
   registration state in that configuration.
3. E2E (Pi harness, compaction off): seed one Visible rule; run a top-level
   session turn; assert the provider request contains `<project-memory>`
   with the rule; assert the response `action` is an additive class and
   `reconcile_pending` is false.
4. E2E parity under compaction off: the OpenCode block and the Pi block are
   equal (cross-record with the parity record).
5. Documentation check: OQ4 answered and reflected in KTD8 before U6.

## Investigation log

### Q: Does the daemon serve m0 when the fold is off at HEAD?
- Sources examined: `transform.rs:3341-3342, 3052, 2790-2832, 11723-11728`.
- Findings: the additive path composes m0 on a `Hard` plan and the serving
  loop emits the frozen m0 unit for non-subagent passes; both are
  independent of fold authority.
- Missing evidence: a test at HEAD proving the additive path's m0 contains
  `<project-memory>` for a seeded row (the plan's U6 step 2 says "add the
  guarantee as a test if absent"; no such test was found in
  `crates/daemon/tests/transform_canonical_memory.rs`, whose four tests do
  not vary fold authority).
- Conclusion: resolved with answer at the code level; unexercised by a test.

### Q: What else does `compactionOff` gate in Pi?
- Sources examined: `index.ts:381-384, 565`.
- Findings: `setEidnaraReduceRegisteredGlobally`, `createPiTransform`, and
  (through `piPassFields`) the `tool_present` field.
- Conclusion: resolvable at implementation - split the three concerns and
  send `tool_present` from the actual registration state.
