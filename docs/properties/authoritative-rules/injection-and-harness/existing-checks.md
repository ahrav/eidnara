# Existing checks: packing primitives, m0 injection path, harness surfaces

HEAD `08de9051a735f136ff63a9dca49eaa11493b3006`. Every check is `unaudited`:
presence is recorded, adequacy is `/testing:invariant-test-review`'s verdict
(tests) or `/low-level-systems:defensive-assertions-and-invariant-guards`'
(production guards). Lines verified at HEAD; test names are the stable
anchors.

## Production assertions and invariant guards

| Location | Semantics | Message or behavior | Status |
| --- | --- | --- | --- |
| `crates/daemon/src/kernel_routes/read.rs:212-217` | A visible decision-kind row with no typed decision at the snapshot fails the read | `KernelError::CorruptCanonicalRow` | unaudited |
| `read.rs:245-252` | Same check after the payload cutoff | `KernelError::CorruptCanonicalRow` | unaudited |
| `crates/daemon/src/m0_compose.rs:79-83` | Budget clamp `.max(1.0)`; wrapper above budget returns empty | silent | unaudited |
| `crates/daemon/src/memory_render.rs:116-122` | Content cut at 64 KiB on a char boundary | silent | unaudited |
| `memory_render.rs:124-141` | Object id escaped and line-folded; content escaped | silent | unaudited |
| `crates/daemon/src/transform.rs:4216-4222` | `is_subagent` forces `Soft`/`Defer` | silent | unaudited |
| `transform.rs:11723-11733` | `m0`/`m1` frozen units served only when `!is_subagent` | silent | unaudited |
| `packages/opencode-plugin/src/shared/memory-mark-command.ts:132-134` | A preview reply that does not answer the operation is `refused/invalid("internal")` | "The reply decoded but does not answer the operation sent" (comment) | unaudited |
| `memory-mark-command.ts:146` | A commit verdict that does not answer the operation is `unavailable("outcome_unknown")` | silent | unaudited |
| `packages/opencode-plugin/src/hooks/context/command-handler.ts:411`, `:420` | Deleted session aborts with the sentinel before and after the runner | `__CONTEXT_MANAGEMENT_<CMD>_HANDLED__` sentinel (`command-handler.ts:119`, `:142`), status 204 | unaudited |
| `packages/pi-plugin/src/index.ts:332-336` | `EIDNARA_PI_SUBAGENT=1` returns before any registration | log line "subagent child detected" | unaudited |
| `index.ts:343-348` | In-process re-init latch | log line "in-process re-init detected" | unaudited |
| `index.ts:573` | Handler returns `undefined` when the project config disables compaction | silent | unaudited |
| `crates/daemon/examples/direct_host_fixture.rs:66`, `:854`, `:867` | Control frame bounded at 64 KiB | `Oversized` | unaudited |

Debug-only assertions in scope: none found in `required.rs`, `scan.rs`,
`canonical_memory.rs`, `m0_compose.rs`, `memory_render.rs`
(`grep -n 'debug_assert' ...` returns nothing for these files).

## Claim-bearing tests

### S3 packing primitives

| Test | Location | Claim | Status |
| --- | --- | --- | --- |
| `every_verdict_maps_to_exactly_one_failure_class` | `crates/retrieval/tests/packing_required.rs:72` | Each kernel verdict yields one `RequiredContextFailure` class | unaudited |
| `stale_rows_and_bounds_are_checked_in_request_order` | `:118` | Request-order fault precedence | unaudited |
| `the_load_bound_is_checked_over_the_whole_set_before_any_request_fault` | `:224` | Load bound before any request | unaudited |
| `reservation_charges_every_byte_and_stops_exactly_at_the_limit` | `:253` | Exactly-at-limit succeeds, one above `OverBudget` | unaudited |
| `the_scan_skips_and_continues_and_the_prefix_packer_does_not` | `crates/retrieval/tests/packing_grouping.rs:352` | Skip-and-continue over 11, 4, 6 at budget 10 | unaudited |

### S4 m0 injection path

| Test | Location | Claim | Status |
| --- | --- | --- | --- |
| `budget_boundaries_match_the_replaced_loop` | `crates/daemon/src/m0_compose.rs:300` | Trim equals the retained loop at NaN, negative, zero, wrapper boundaries, fractional, 1e30, +inf | unaudited |
| `skip_and_continue_delegation_matches_the_replaced_loop` | `:337` | Proptest over 0..12 rows in three categories, budgets NaN and -50..400 | unaudited |
| `injectable_rows_are_visible_decisions_in_a_positive_category` | `crates/daemon/src/canonical_memory.rs:282` | Visible, decision present, positive kind | unaudited |
| `revision_changes_only_when_the_rendered_inputs_change` | `:315` | Revision independent of `known_as_of`, order, `truncated`; moves on edit, add, empty | unaudited |
| `rows_the_renderer_drops_do_not_move_the_revision` | `:373` | Non-positive rows not digested | unaudited |
| `bytes_past_the_render_cap_do_not_move_the_revision` | `:390` | Bytes past 64 KiB not digested; bytes inside are | unaudited |
| `rows_past_the_memory_budget_are_neither_injected_nor_digested` | `:412` | Budget-dropped row absent and undigested; newer oversized row loses to older short row | unaudited |
| `withheld_reads_record_the_verdict_and_inject_nothing` | `:457` | Withheld differs from empty | unaudited |
| `render_boundary_drops_non_positive_categories` | `crates/daemon/src/memory_render.rs:619` | Renderer drops REJECTED_APPROACH and unknown names | unaudited |
| `object_id_markup_and_line_breaks_cannot_forge_block_structure` | `:646` | Escaping and line folding | unaudited |
| `content_is_cut_at_the_char_boundary_before_the_64_kib_cap` | `:670` | Cut position | unaudited |
| `positive_category_vocabulary_matches_the_frozen_typescript_arrays` | `:714` | Twelve names equal the frozen fixture; writable taxonomies are positive | unaudited |
| `render_order_is_a_prefix_of_the_positive_vocabulary` | `:737` | Rank order aligned with inclusion | unaudited |
| `canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot` | `crates/daemon/src/transform.rs:13851` | Block rendered; response and meta record the composition | unaudited |
| `withheld_canonical_read_composes_no_block_and_differs_from_empty_memory` | `:13894` | Stale, abstained, unavailable produce `Withheld`; not equal to empty. Runs with `eidnara_folds: true` | unaudited |
| `project_memory_revision_change_requests_a_hard_fold` | `:13958` | Changed revision yields HARD with `project_memory_epoch`; equal revision does not | unaudited |
| `project_memory_epoch_from_state_sync_is_an_eager_hard_input` | `:17437` | Out-of-process epoch marks a pending HARD | unaudited |
| `transform_composes_project_memory_from_canonical_rows_and_observes_absence` | `crates/daemon/tests/transform_canonical_memory.rs:46` | Real store: quarantined, retired, superseded, other-project, other-domain, plain-kind absent; quarantine between passes refolds; unknown `claim_lane` ignored | unaudited |
| `a_lagging_consumer_withholds_the_block_and_acknowledging_restores_it` | `:241` | Lag withholds; ack restores | unaudited |
| `a_memory_disabled_pass_takes_no_canonical_read` | `:320` | Disabled: no block, no `project_memory` member | unaudited |
| `rows_past_the_configured_budget_are_dropped_by_the_reader` | `:356` | 500-token user budget drops the long row; editing it holds the revision | unaudited |
| `the_memory_block_matches_the_replace_chain_renderer` | `memory_render.rs:446` | Renderer equals a reference | unaudited |

### S5 harness surfaces

| Test | Location | Claim | Status |
| --- | --- | --- | --- |
| `parseMemoryMarkArgs` (2 tests) | `packages/opencode-plugin/src/shared/memory-mark-command.test.ts:67-90` | Event spelling, flag position, refusals | unaudited |
| `runMemoryMarkCommand` (12 tests) | `:92-290` | Confirmation gate, decline, denial, replay, cancellation, foreign verdict, refused preview | unaudited |
| Pi `/eidnara-memory-mark` (5 tests) | `packages/pi-plugin/src/commands/eidnara-memory-mark.test.ts:63-150` | Dialog, replaced session, no-UI fallback to `--yes`, denial | unaudited |
| OpenCode `eidnara-memory-mark` (4 tests) | `packages/opencode-plugin/src/hooks/context/command-handler.test.ts:757-850` | No dialog, `--yes`, toast cut, disabled client; sentinel asserted | unaudited |
| `no-ops before registering anything inside Eidnara Pi subagents` | `packages/pi-plugin/src/index-env-guard.test.ts:38` | Env guard | unaudited |
| `registers the full runtime when the subagent guard is absent` | `:54` | Registration | unaudited |
| In-process latch (3 tests) | `packages/pi-plugin/src/index-in-process-latch.test.ts:43-100` | Second init no-op; dispose re-arms | unaudited |
| `pi-pass-inputs.test.ts` (4 tests) | `packages/pi-plugin/src/transform/pi-pass-inputs.test.ts:39-95` | Usage and completion time fields | unaudited |
| restored child sends `is_subagent: true` | `packages/opencode-plugin/src/hooks/context/hook.test.ts:1790-1803` | OpenCode flag derivation | unaudited |
| `credits C3.M1 only when admitted and included ...` | `packages/e2e-tests/tests/compression-fidelity-memory.test.ts:77-255` | Seeded Verified row renders; wrong-project, candidate, budget-excluded, quarantined, rejected absent; `memorySeed` returns `verified`/`automatic` | unaudited |
| `serves a scripted C1 publication at P1 in m1 and then in a rematerialized m0` | `packages/e2e-tests/tests/compression-fidelity-pi.test.ts:155` | Pi receives m0 under compaction on | unaudited |

## Categories with none found

- Subagent request serving no `<project-memory>`: none found.
- Additive-only (`eidnara_folds = false`) pass rendering the block: none
  found.
- Pi `is_subagent: false` on the wire: none found.
- Pi `compactionOff` registering no `context` handler: none found.
- Trim cost model against the renderer or the production tokenizer: none
  found.
- `project_memory.truncated` under budget pressure: none found.
- `memorySeed` row's `source_kind` or admission classes read back: none
  found (the e2e asserts maturity and visibility only).
- TypeScript default of 4,000 tokens matching `DEFAULT_MEMORY_BUDGET_TOKENS`:
  none found.
- Wire shape of `project_memory` diagnostics in `docs/host-wire-protocol.md`:
  none found (listed at `:1032` without a shape).

## Suspiciously quiet areas

1. `transform.rs:11723` is the single line that keeps m0 from subagents, and
   no test names it; the plan's parity contract turns on this flag.
2. `compose_additive_m0` (`transform.rs:2790-2832`) duplicates
   `compose_m0`'s memory handling with its own gate; the two can drift and
   only the fold path is tested for the block.
3. `index.ts:565` and `:573` are two independent compaction gates on the Pi
   handler with no test for either; U6 lifts them.
4. The trim's cost model is tested only against itself (the retained loop);
   nothing ties it to `render_memory_block`.
5. `DefaultHasher` revision persistence across daemon versions is unexamined.
