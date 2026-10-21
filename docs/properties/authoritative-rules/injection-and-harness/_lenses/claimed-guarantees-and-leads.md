# Lens: claimed guarantees (safety and liveness) and contract-versus-code

Claims are quoted or paraphrased from doc comments, catalogs, the plan, and
the tracker. Each stays a claim under test. Lines verified at HEAD.

## Claims from code comments

| # | Claim | Source | Code reading |
| --- | --- | --- | --- |
| C1 | "A pass reads once and composes every memory surface of that pass from the same pinned rows." | `canonical_memory.rs:3-6` | One read at `lib.rs:9345`; the summarizer firing a pass triggers receives `project_memory.cloned()` (`lib.rs:5820`). The wrapup path takes its own read (`lib.rs:6006`); it is not a pass. Agrees. |
| C2 | "The reader trims the rows to the pass's memory budget, so a snapshot holds exactly the rows the `<project-memory>` block renders." | `canonical_memory.rs:5-6` | `injectable_snapshot` trims; `compose_m0` renders every row given (`m0_compose.rs:176-178`). Agrees. |
| C3 | "Rows are digested in object-id order after the renderer's positive-category filter ... an edit past the renderer's content cap changes no digested byte. `truncated` is not digested." | `canonical_memory.rs:42-49` | Agrees with `:50-67`. Tests `:315-370`, `:373-410`. |
| C4 | "Labeled rows never reach `auto_inject`; the guard keeps a label-bearing row out of a block that has no label renderer." | `canonical_memory.rs:178-181` | `.filter(visibility == Visible)` at `:197`. Agrees. |
| C5 | "Keeps the memories whose rendered lines fit `budget_tokens`, in the supplied serving order, charging each category's wrapper once. A row that does not fit is skipped." | `m0_compose.rs:70-73` | Agrees with `:74-112`. The supplied order is `read_visible` newest-first. |
| C6 | "`truncated` records the kernel read's row or byte cap over the project's visible rows, not the token budget the block was trimmed to." | `memory-store/src/lib.rs:2170-2173` | `read.rs:197`, `:233` set it; the trim sets nothing. Agrees. No diagnostic names the budget-dropped count. |
| C7 | "`DEFAULT_MEMORY_BUDGET_TOKENS` must equal the TypeScript schema default of 4,000 tokens." | `config.rs:30-31` | Test `config.rs:1807-1808` pins 4000. The TypeScript side is not checked by that test. |
| C8 | "`None` when memory is disabled, so a disabled deployment never touches the kernel store for it." | `lib.rs:5444-5446` | `cfg.memory_enabled.then(...)` at `:5453`. Agrees. |
| C9 | "Disabled memory removes both project memories and the user-profile memory block." | `m0_compose.rs:62` | `:209`, `:211-215`. Agrees. |
| C10 | "The module omits `descent_edge_id` on ordinary, subagent ... responses"; subagent passes are Soft or Defer. | `transform.rs:1489`, `:4216-4222` | Agrees. |
| C11 | `/eidnara-memory-mark` "names a kernel `EventKind`, never a target disposition ... the harness asks for confirmation only when a serving surface would display a different state after the event." | `memory-mark-command.ts:1-7` | `:135-138`. Agrees. |
| C12 | "Pi registers tools once per process, so compaction registration does not follow later /cd config changes." | `index.ts:381` | `:383`, `:565`. Agrees. |
| C13 | "Unknown fields are ignored and never read" on the transform request. | `docs/host-wire-protocol.md:951` | `transform_canonical_memory.rs:206-214` sends `claim_lane`; served bytes and `project_memory` equal the prior pass. Agrees. |

## Claims from the plan that depend on existing behavior

| # | Plan statement | Code reading | Disposition |
| --- | --- | --- | --- |
| P1 | KTD6: "`injectable_snapshot` maps `source_kind = user_directive` to the `USER_DIRECTIVES` heading and keeps `decision_kind` as category for every other row." | Today the category is `decision_kind` only (`canonical_memory.rs:199-203`); `adr_accepted` is not positive, so such a row is dropped. `USER_DIRECTIVES` is already positive (`memory_render.rs:72`) and renders after the five ranked categories. | Existing behavior cataloged; the plan changes it. |
| P2 | KTD8: "the daemon serves PASSTHROUGH plus m0 when the fold is off." | `apply_additive_only` -> `compose_additive_m0` appends `<project-memory>` (`transform.rs:2790-2832`). No test asserts the block on that path. | Implemented, unexercised. Lead L2. |
| P3 | KTD8: Pi "splits `createPiTransform` registration from `compactionOff`". | Registration gated at `index.ts:565`; handler also returns at `:573` when the per-project config disables compaction. Two gates, not one. | Existing behavior cataloged; the plan must remove both. Lead L3. |
| P4 | KTD8: "Pi keeps `is_subagent: false`". | `pi-pass-inputs.ts:110`. No Pi test asserts it. | Cataloged. |
| P5 | U6: "`memorySeed` for the directive row". | `memory_seed` writes `source_kind = "repo"` (`lib.rs:13814`) and admits `CodeObserved` with `(TrustedLocalCode, CurrentCode)` (`direct_host_fixture.rs:777-782`). It cannot produce `source_kind = user_directive` or an `AcceptedAdr` event. | Contract-versus-seam disagreement. Lead L4. |
| P6 | U2: "Convert the budget to an integer once." | `trim_memories_to_budget` applies `.max(1.0)` then `floor() as u64` (`m0_compose.rs:79`, `:90`); `packing-budget-is-an-integer-never-clamped` names this clamp as the precedent the packer must not inherit. | The plan reuses `required` for the memory trim; which budget conversion wins is unstated. Lead L5. |
| P7 | KTD5: "Disclosure ... N included in the snapshot revision digest" and R11. | Digest covers categories and rendered lines only (`canonical_memory.rs:57-61`). Adding N is a digest input change; the literal domain tag `eidnara-project-memory-revision-v1` would then describe a different byte layout. | Lead L6: the tag should move with the layout. |
| P8 | KTD7: `project_memory` diagnostics "gain `rules_served`, `rules_omitted`, `context_resolved`". | The wire document lists `project_memory` with no shape (`host-wire-protocol.md:1032`); the shape is Rust-only (`memory-store/src/lib.rs:2168-2181`). | Lead L7: the current shape is an undocumented contract. |
| P9 | R13 parity: "Pi registers its context handler whenever the project binds". | Pi also skips registration for `EIDNARA_PI_SUBAGENT=1` children and in-process re-inits (`index.ts:332-348`). | In-process pi-subagents children: unresolved whether the parent's `context` handler observes child sessions. Lead L8. |

## Claims from the existing catalogs

- `packing-optional-scan-skips-and-continues` (grouping catalog): the memory
  trim "matches its replaced loop on boundary budgets and generated rows".
  Agrees with `m0_compose.rs:257-355`.
- `canonical-read-staleness-is-distinguishable-from-emptiness` (daemon
  transform catalog) says the unit test drives "the compaction-off engine".
  The test `withheld_canonical_read_composes_no_block_and_differs_from_empty_memory`
  uses `pctx` with `FoldAuthorityIntent::default()` whose `eidnara_folds`
  is `true` (`fold_authority.rs:11-19`, `transform.rs:13803`). The wording
  in that catalog overstates the path exercised. Lead L2 again.
- `cf-memory-credit-requires-admitted-visible-content`: "a verified row of
  64 KiB spaces followed by its qualifier is admitted with the qualifier cut
  off"; `memory-seed` frame cap 64 KiB (`direct_host_fixture.rs:66`).
  Agrees with `memory_line_content` (`memory_render.rs:116-122`).

## Liveness claims

None in scope. The trim and render are pure; the read is one-shot per pass.
A revision change requests a HARD on the next pass
(`project_memory_revision_change_requests_a_hard_fold`, `transform.rs:13958`),
which is a per-pass obligation, not an eventual one.
