# Lens: architecture and data flow (S3, S4, S5)

HEAD 08de9051a735f136ff63a9dca49eaa11493b3006. Every line verified at HEAD.

## S3 packing primitives (crates/retrieval/src/packing)

- `required.rs:13-31` `TokenCount` trait: integer accounting with
  `checked_add`/`checked_sub`; `u64` is the only impl in the crate.
- `required.rs:114-166` `admit_required`: load-count bound checked over the
  whole set first (`facts.get(max_payload_loads)`), then per request in
  order: occurrence mismatch -> Corrupt, stale revision -> Stale, kernel
  disposition Hidden -> Hidden, Stale|Superseded -> Stale, other excluded ->
  Ineligible, item bytes -> Oversized(ItemBytes), running total ->
  Oversized(PayloadBytes). No byte loaded.
- `required.rs:178-206` `reserve_required`: refuses Corrupt when fewer byte
  slices than admitted items or a length mismatch; sums costs through
  `checked_add`; `Some(sum) if sum <= token_limit` continues, otherwise
  `OverBudget { limit, charged: sum.unwrap_or(C::MAX) }` at the crossing
  item (`:196-201`). Exactly-at-limit succeeds; one above fails.
- `scan.rs:18-43` `skip_and_continue`: visits every index once, deducts
  `marginal(state, index)` with `checked_sub`; on `None` pushes to `skipped`
  and continues; `admit` runs after deduction so later marginals price
  against admitted state.
- Production callers: `admit_required`/`reserve_required` have none
  (`grep -rn 'reserve_required\|admit_required' crates --include=*.rs`
  finds `crates/daemon/src/packing/mod.rs` and tests only; `prepare_required`
  is called from `crates/daemon/tests/packing_required.rs` only).
  `skip_and_continue` has one production caller: `m0_compose.rs:88`.

## S4 m0 injection path (crates/daemon)

Data flow per pass (`lib.rs:9345`): `Handler::project_memory_read`
(`lib.rs:5447-5461`) returns `None` when `cfg.memory_enabled` is false,
else `canonical_memory::read_project_memory(kernel, binding.kernel_project,
pass_now, cfg.memory_budget_tokens)`. The same function serves the
history-summarizer fire at `lib.rs:6006`.

`read_project_memory` (`canonical_memory.rs:141-181`): kernel store phase ->
`tip()` -> `outbox_lag` -> `serving::decide_for_tip_read` projected on
`Surface::AutoInject`; any non-available verdict -> `Withheld(outcome)`.
Then `read_visible(store, project, AutoInject, Some(tip),
RowSelection::DomainDecisions("memory"))` (`:163-169`).

`read_visible` (`kernel_routes/read.rs:163-258`): project scope term pushed
into the kernel query; `ScopeFilter::matches` per row; `NewestRows` keeps at
most `MAX_READ_ROWS = 8192` in serving order `created_commit_seq` desc,
`object_id` asc (`:99`, `:127`); payload-size cutoff at
`MAX_READ_ROW_BYTES` (`:229-234`) sets `truncated`; a decision row without
its typed decision at the snapshot -> `KernelError::CorruptCanonicalRow`
(`:212-217`, `:250-253`).

`injectable_snapshot` (`canonical_memory.rs:184-211`): keeps rows with
`visibility == Visible`, with a decision row present, and
`is_positive_memory_category(decision_kind)`; maps to `CanonicalMemory
{object_id, category: decision_kind, content: payload.summary}`; then
`trim_memories_to_budget(&injectable, memory_budget_tokens,
cached_estimate_tokens)`; then `CanonicalMemorySnapshot::new`.

`trim_memories_to_budget` (`m0_compose.rs:74-112`): `budget =
budget_tokens.max(1.0)`; wrapper cost `estimate("<project-memory>\n</project-memory>")`;
returns empty when wrapper > budget; integer budget
`budget.floor() as u64 - wrapper as u64`; `skip_and_continue` over positive
rows in supplied order; marginal = line + "\n" plus, once per category,
`<CAT>\n</CAT>\n`.

`CanonicalMemorySnapshot::new` (`canonical_memory.rs:50-67`): digest
`DefaultHasher` seeded with literal `eidnara-project-memory-revision-v1`,
over positive rows sorted by `object_id`, hashing `category` then
`render_memory_line(row)`. `known_as_of` and `truncated` are not digested.

`compose_m0` (`m0_compose.rs:179-254`): `selected_memories = memories` only
when `inputs.memory_enabled` (`:209`); `render_memory_block(selected,
"project-memory")` appended after `\n\n` when non-empty (`:239-243`).
`compose_additive_m0` (`transform.rs:2790-2832`) does the same for the
fold-off path (`apply_additive_only`, entered at `transform.rs:3341` when
`!authority.eidnara_folds`).

`render_memory_block` (`memory_render.rs:157-213`): drops rows whose
category has no rank (`memory_category_rank`, `:91-96`), sorts by rank then
category name (for the unranked positive tail) then `object_id`; emits
`<project-memory>` + per category `\n<CAT>` lines `\n</CAT>` +
`\n</project-memory>`. `render_memory_line` (`:144-149`) cuts content at
64 KiB on a char boundary (`:116-122`) and escapes `& < >`; object ids are
escaped and line-folded (`:124-141`).

Vocabulary (`memory_render.rs:59-80`): `MEMORY_CATEGORY_ORDER` five ranked
names; `POSITIVE_MEMORY_CATEGORIES` twelve names including
`USER_DIRECTIVES`. `adr_accepted` is absent, so a decision with
`decision_kind = adr_accepted` is dropped at `canonical_memory.rs:199`.

Record and diagnostics: `ProjectMemoryComposition` (`memory-store/src/lib.rs:2168-2181`)
`Canonical { known_as_of, truncated, revision }` | `Withheld { state }`;
written to `TransformResponse.project_memory` (`transform.rs:1488`) and
`HistorySummarizerDiagnostics.project_memory` (`:1614`). The wire document
lists `project_memory` among optional diagnostics
(`docs/host-wire-protocol.md:1032`) without a shape.

m1 revision signal (`m1_compose.rs:61-92`): `memory_revision =
memory_enabled.then_some(project_memory_revision).flatten()` hashed into
both the in-session and external revisions; a change in the external
revision requests a HARD with `materialize_reason = "project_memory_epoch"`
(`transform.rs:12436`, test `:13958`).

Subagent gating: `transform.rs:4216-4222` forces `PassPlan::Soft|Defer`
when `req.is_subagent`; `:11723-11733` pushes the `m0`/`m1` frozen units
only when `!req.is_subagent`.

Tool-call input carriage: `BlockKind::ToolCall { id, name, input: Value,
provider_executed }` (`memory-store/src/lib.rs:211-218`); `sel_item_from_flat`
(`transform.rs:6874-6889`) borrows `input` as `Cow::Borrowed` without
parsing. No path extraction exists today.

## S5 harness surfaces

- Pass fields: `transform-pass-fields.ts:55-100` `transformPassFields`
  shared by both harnesses; `is_subagent: args.passInputs.is_subagent === true`
  (`:66`). Pi `pi-pass-inputs.ts:110` passes `is_subagent: false` always;
  OpenCode `opencode-transform-adapter.ts:458, 507` passes
  `deps.isSubagentSession(sessionId)`.
- Pi registration: `index.ts:332-336` returns before registration when
  `EIDNARA_PI_SUBAGENT=1`; `:343` in-process latch; `:382-384`
  `compactionOff = !PI_TRANSFORM_AVAILABLE || !compactionRequested`;
  `:565` `piTransform = compactionOff ? undefined : createPiTransform(...)`;
  `:573` handler returns `undefined` when `!isCompactionEnabled(deps.config)`;
  `:550` `registerEidnaraMemoryMarkCommand`. `subagent-entry.ts` registers
  tools only (`registerEidnaraTools`), no `registerCommand`.
- Memory-mark runner (`memory-mark-command.ts:117-158`): preview, then
  confirm gate (`:135-138`), cancellation re-check (`:140`), commit (`:142`);
  operation identity `[sessionId, event, objectId]` (`:98-100`). Pi wrapper
  (`eidnara-memory-mark.ts:65-72`) passes `confirm` only when `ctx.hasUI`;
  OpenCode `command-handler.ts:393-422` has no dialog, aborts the model turn
  through `throwSentinel` (`:411`, `:420`, `:713`).
- Shared client response: `transform-session-client.ts:464-468`
  `responseValue` returns `Record<string, unknown>`; no TypeScript type
  names `project_memory`.
- e2e seams: `hermetic-host.ts:768-777` `memorySeed(anchor, objectId,
  category, text)` -> fixture `memory_seed` (`direct_host_fixture.rs:757-783`)
  inserts `memory_decision_spec` (source_kind `"repo"`, `lib.rs:13814`) and
  admits through `observe_code` with `(TrustedLocalCode, CurrentCode)`;
  `cache-analysis.ts:307-309` `extractM0` returns the whole text of the
  first message containing `<session-history>`.
