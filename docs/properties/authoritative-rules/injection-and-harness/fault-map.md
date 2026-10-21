# Fault map: packing primitives, m0 injection path, harness surfaces

HEAD `08de9051a735f136ff63a9dca49eaa11493b3006`. Faults are what a test must
construct so the record is not vacuous. Coverage checks assert preconditions
and fire on a correct system.

## Fault classes and availability

| Class | How to construct | Available today |
| --- | --- | --- |
| F1 Non-positive or forbidden decision kind | Commit a decision with `decision_kind` in {`adr_accepted`, `REJECTED_APPROACH`, `memory`} and verify it (`commit_verified_memory`, `tests/transform_canonical_memory.rs:72-74`) | yes, integration seam |
| F2 Labeled row in a positive kind | A candidate row (`eidnara_memory create`) or a `ReadResponse` built with `SurfaceVisibility::Labeled` (`canonical_memory.rs:288`) | yes, unit and e2e |
| F3 Missing typed decision at snapshot | Needs a store with a visible decision-kind row and no decision row at `known_as_of`; no fixture constructs it | no (needs a kernel-level corruption seam) |
| F4 Budget below one row, exact boundary | Byte estimator and a budget of `wrapper + cost(first)` (`m0_compose.rs:300`) | yes, unit |
| F5 Malformed budget | Direct call with NaN, negative, zero, fractional, +inf | yes, unit only; config clamps first |
| F6 Recency inversion across categories | Newer NAMING row then older PROJECT_RULES row, one-row budget | yes, unit (`canonical_memory.rs:412` pattern) |
| F7 Rows with escapable bytes, newlines, 64 KiB content | Construct `CanonicalMemory` directly | yes, unit |
| F8 Production tokenizer | `token_cache::cached_estimate_tokens` or `tokenizer::estimate_tokens` in a unit test | yes, unit |
| F9 Kernel read caps | More than 8192 visible rows or 8 MiB of payload (`read.rs:26`, `:29`) | yes but costly; a smaller cap is not injectable |
| F10 Kernel commit between two passes | `store.commit(...)` between `daemon.call` (`tests/transform_canonical_memory.rs:159-170`) | yes, integration |
| F11 Subagent request | `request.is_subagent = true` (`transform.rs:12599` pattern) | yes, unit |
| F12 Native fold authority | `ctx.fold_authority.eidnara_folds = false` (`transform.rs:16293`) or an empty `model_chain` | yes, unit; default config |
| F13 Pi compaction off | `compaction.enabled: false` in the Pi config fixture; `ExtensionAPI` stub recording `on` calls (`start-notice-order.test.ts:28` shape) | yes, unit |
| F14 Pi child session | pi-subagents in-process child; `EIDNARA_PI_SUBAGENT=1` env | partial: env guard testable; in-process handler scope not observable here |
| F15 Preview with visibility change, replay receipt, denial, non-available state | Fake `KernelClient` (`memory-mark-command.test.ts:1-66`) | yes, unit |
| F16 Session deleted or replaced mid-command | `isSessionDeleted` / `ctx` invalidation fakes | yes, unit |
| F17 Seeded row readback | `memorySeed` then `kernel.read` on `explicit_search` through the test kernel client | yes, e2e (unused today) |
| F18 Toolchain hasher change | Not injectable; reasoned only | no |

## Per-property required faults and enabling state

| Record | Required faults | Enabling state |
| --- | --- | --- |
| injectable-rows-require-visible-verdict-and-positive-decision-kind | F1, F2; F3 for the fail-closed arm | `memory_enabled`, served read, project scope bound |
| memory-trim-admits-in-serving-order-not-category-order | F4, F6 | two positive rows of different categories, newer in a lower rank |
| memory-trim-cost-model-mirrors-rendered-block-structure | F7; F8 for the token arm | kept set of at least two categories |
| memory-trim-clamps-malformed-budget-and-floors-the-integer | F5 | direct call; byte estimator |
| snapshot-revision-digests-only-rendered-rows | F10 (quarantine, edit inside the cut, edit past the cut, edit past the budget); two reads differing in `known_as_of` | a HARD already frozen so `m1_external_revision` is non-zero (`transform.rs:2907-2908`) |
| project-memory-truncated-flag-excludes-budget-trimming | F4 for the `false` arm; F9 for the `true` arm | served read |
| subagent-pass-is-never-hard-and-serves-no-m0 | F11; F12 for the additive-path arm | Available read with one positive row; `memory_enabled`; both fold-authority values |
| additive-only-pass-still-composes-project-memory | F12 | Available read with one positive row; `memory_enabled`; a scheduler or config event that materializes |
| pi-context-handler-is-gated-by-compaction-on | F13 | Pi factory run against a recording `ExtensionAPI` stub |
| pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false | F14 for the child arm; a non-boolean `is_subagent` input | both builders invoked with the same `passInputs` |
| memory-mark-command-is-model-free-and-confirms-before-writing | F15, F16 | fake client; Pi `ctx.hasUI` true and false; OpenCode handler with `isSessionDeleted` |
| memory-seed-fixture-writes-repo-source-kind-under-code-observed | F17; a text at the 64 KiB frame cap | running fixture with a bound anchor |

## Coverage checks to add (preconditions only)

Names are constant and globally unique. Each fires on a correct system.

- `m0_injectable_row_dropped_by_kind`: a Visible decision row with a
  non-positive kind was present in the read (precondition of the filter
  record).
- `m0_labeled_row_in_positive_kind_present`: a Labeled row with a positive
  kind was present.
- `m0_trim_skipped_then_admitted`: the trim skipped at least one row and
  admitted a later one in the same call (the skip-and-continue window).
- `m0_trim_recency_inversion_present`: the admitted set contains a row of
  lower category rank than a skipped row with an older `created_commit_seq`.
- `m0_trim_two_categories_kept`: at least two categories survived, so the
  per-category wrapper charge was exercised more than once.
- `m0_budget_dropped_row`: the trim dropped at least one row on a served
  read (precondition of the flag record; the record asserts the flag's
  value, this marks only that the pressure happened).
- `m0_revision_compared_against_frozen`: a pass compared a non-`None`
  `project_memory_revision()` with a non-zero `m1_external_revision`.
- `transform_subagent_request_with_injectable_rows`: a request with
  `is_subagent: true` arrived while the pinned read had at least one row.
- `transform_additive_only_materialized_with_rows`: the additive-only path
  materialized while the pinned read had at least one row and
  `memory_enabled` was true.
- `pi_factory_ran_with_compaction_off`: the Pi factory ran with
  `compactionOff == true`.
- `pi_pass_fields_built_for_child_session`: `piPassFields` ran for a
  session that pi-subagents created in-process.
- `memory_mark_preview_reported_visibility_change`: a preview returned
  `visibility_changes: true` (precondition of the confirmation gate).
- `memory_mark_session_ended_between_preview_and_commit`: `isCancelled()`
  flipped after the preview resolved.
- `memory_seed_row_read_back`: a seeded row was read through `kernel.read`
  after seeding.

## Ranking by cheapest valid oracle

1. `memory-trim-cost-model-mirrors-rendered-block-structure`: one unit test
   comparing a sum with `render_memory_block(...).len()`; pure functions,
   byte estimator, exact oracle. Highest value for KTD5's reserve math.
2. `subagent-pass-is-never-hard-and-serves-no-m0`: one `transform.rs` unit
   test flipping `request.is_subagent` on the existing
   `canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot`
   fixture; asserts bytes and `action`.
3. `additive-only-pass-still-composes-project-memory`: same fixture with
   `ctx.fold_authority.eidnara_folds = false`; closes the KTD8 assumption
   before U6.
4. `memory-trim-admits-in-serving-order-not-category-order`: extend the
   `canonical_memory.rs:412` pattern with two categories.
5. `project-memory-truncated-flag-excludes-budget-trimming`: add one
   assertion to `rows_past_the_configured_budget_are_dropped_by_the_reader`.
6. `pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false`: one
   `pi-pass-inputs.test.ts` case plus a key-set equality test over both
   builders.
7. `pi-context-handler-is-gated-by-compaction-on`: a recording
   `ExtensionAPI` stub; medium effort because the factory has many
   dependencies.
8. `memory-seed-fixture-writes-repo-source-kind-under-code-observed`: an
   e2e readback; costly (native addon job) but the only oracle.
9. `injectable-rows-require-visible-verdict-and-positive-decision-kind`:
   mostly covered; the `adr_accepted` case is one more row in the existing
   integration test.
10. `snapshot-revision-digests-only-rendered-rows`: covered; the hasher
    question needs a human, not a test.
11. `memory-trim-clamps-malformed-budget-and-floors-the-integer`: covered;
    the open question is a design decision.
12. `memory-mark-command-is-model-free-and-confirms-before-writing`: well
    covered; add a static check that `subagent-entry.ts` registers no
    command.
