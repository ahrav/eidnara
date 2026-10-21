# additive-only-pass-still-composes-project-memory

## Discovery trigger

Lens: claimed guarantees (P2) and existing tests. KTD8 says "the daemon
serves PASSTHROUGH plus m0 when the fold is off" and U6 step 2 says "Confirm
the daemon serves PASSTHROUGH plus m0 when the fold is off; add the
guarantee as a test if absent". Reading `apply_additive_only` confirmed the
composition and found no test for the block on that path.

## Evidence trail

- Entry: `crates/daemon/src/transform.rs:3341` `if !authority.eidnara_folds
  && !(req.lineage_switched && req.is_subagent) { return
  apply_additive_only(...) }`.
- Fold authority: `crates/daemon/src/config.rs:182-186` `eidnara_folds()`
  is `admission == Admitted && compaction_enabled && !model_chain.is_empty()`;
  defaults `compaction_enabled: true` (`:145`), `model_chain: Vec::new()`
  (`:143`). A deployment without `history_summarizer.model` is on this path
  by default. Bound at `lib.rs:305` (`eidnara_folds: binding.config.eidnara_folds()`).
- Composition: `transform.rs:2790-2832` `compose_additive_m0`:
  `selected_memories = if ctx.memory_enabled { ctx.project_memory_rows() }
  else { &[] }`; renders docs and profile with no history; appends
  `render_memory_block(selected_memories, "project-memory")` after `\n\n`
  when non-empty.
- Trigger inputs: `:2898-2910` `revision_signal_for_context` folds the
  pinned read's revision; `external_revision_changed` and
  `project_memory_epoch_hard_due` feed `activation_gates` (`:2975-2990`) and
  the bust opportunity (`:2991-2995`).
- Materialization: `:3050-3075` on `Hard | MigrateHard` composes the
  additive m0 and freezes `m0` and `m1` units; `:3178-3196` serves them
  before the request messages on every additive response.
- Record: the additive HARD writes `meta.project_memory =
  ctx.project_memory_composition()` (`:3101`) and the additive response
  carries `meta.project_memory.clone()` (`:3313`).
- Tests with `eidnara_folds = false`: `transform.rs:16293`, `:16438`,
  `:16521`, `:16586`; none asserts `<project-memory>`.
- The transform catalog's `canonical-read-staleness-is-distinguishable-from-emptiness`
  says `transform.rs:13894` runs "through the compaction-off engine"; that
  test's `pctx` (`:13803`) uses `FoldAuthorityIntent::default()` whose
  `eidnara_folds` is `true` (`crates/daemon/src/fold_authority.rs:11-19`).
  The wording overstates the path exercised.

## Failure scenario

U6 lifts Pi's compaction gate expecting rules in m0. If the additive path
stopped composing the block (a refactor that shares `compose_m0` and passes
an empty slice, or a gate copied from the fold path), compaction-off Pi
users would receive passthrough with an m0 that lacks `<project-memory>`
while the fold path still serves it, and AE8 parity would hold only for
compaction-on sessions.

## Timing windows and dependencies

The additive path materializes on `Hard | MigrateHard` from `classify`,
driven by scheduler, configuration, render-config, revision, or epoch
events; a revision change between such events is served on the next
materialization. Depends on `memory_enabled` from the binding config.

## What a test must construct

1. On the `canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot`
   fixture (`transform.rs:13851`), set `ctx.fold_authority.eidnara_folds =
   false`; assert `action == "HARD"`, served messages begin with a synthetic
   m0 containing `<project-memory>` with the row, and `response.project_memory`
   is `Canonical`.
2. Set `ctx.memory_enabled = false`; assert the block is absent and
   `project_memory` is `None`.
3. Change the row text and run a second pass; assert a refold with
   `materialize_reason: "project_memory_epoch"` on the additive path.
4. Coverage: `transform_additive_only_materialized_with_rows` fires.

## Investigation log

### Q: Is any existing test a witness of the block on the additive path?

- Sources examined: every `eidnara_folds: false` site in `transform.rs`
  (`:16293`, `:16438`, `:16521`, `:16586`); `grep -n 'project-memory'` over
  `crates/daemon/tests/*.rs` (only `transform_canonical_memory.rs`, which
  uses the default fold authority).
- Findings: none asserts the block with the fold off.
- Missing evidence: the test in item 1.
- Conclusion: resolved with answer: no witness exists; the record is
  `Exercised: not yet`.
