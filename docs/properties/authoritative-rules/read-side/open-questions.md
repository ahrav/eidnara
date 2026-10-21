# Open questions: proposed read side (N3, N4, N5)

Each question states what the plan and the code at HEAD say, why this pass
could not settle it, the tag, and the evidence that would resolve it. The
accepted design is preserved; where a question implies a design change it
is flagged as such and left to the owner.

## Needs human input

### OQ-RS-1 OpenCode has no confirmation dialog on the command path

- Plan: U4 step 4 "Refuse to run without a UI confirm (guard against
  `sendUserMessage`-style queued text)"; U4 step 1 mirrors the memory-mark
  runner.
- Code: `packages/opencode-plugin/src/hooks/context/command-handler.ts:389`
  "OpenCode's command path has no dialog, so the confirmation is the reply
  itself: the user re-issues the command with the confirm flag"; the runner
  receives no `confirm` callback (`:411-417`).
- Why unsettled: as written, the gate makes directive authoring impossible
  on OpenCode, which contradicts R1 ("a user-invoked command in each
  harness") and AE1's OpenCode e2e in U4.
- Resolution evidence: an OpenCode plugin API that can show a dialog, or a
  decision to accept the two-step textual confirmation on OpenCode (and to
  state why queued text cannot reach `command.execute.before`), or to make
  authoring Pi-only. Records affected:
  `parity-directive-command-writes-only-after-ui-confirmation`,
  `parity-directive-command-dispatch-is-host-user-only`.

### OQ-RS-2 Required-first packing versus category-grouped rendering

- Plan: U2 test scenario "Required rows render before optional rows
  regardless of recency"; R10 speaks of packing priority ("charges them
  untruncated in a required phase ... before any other memory is packed").
- Code: `render_memory_block` orders by category rank then object id
  (`crates/daemon/src/memory_render.rs:85-109, 157-208`); `USER_DIRECTIVES`
  is rank 5, after `NAMING`, so a directive renders after every
  `ARCHITECTURE` or `CONSTRAINTS` memory.
- Why unsettled: "render before" and "pack before" differ; satisfying the
  test scenario literally requires a renderer change the plan does not list.
- Resolution evidence: a decision whether the scenario means packing
  priority only (no renderer change; the record set here assumes this) or
  render position (then `MEMORY_CATEGORY_ORDER` or the block layout changes
  and `render-a-composition-order-is-fixed-and-each-unit-appears-once` in
  `docs/properties/daemon/rendering/catalog.md:472` is affected). Records:
  `packing-required-order-is-total-and-identical-across-passes`.

### OQ-RS-3 Read cap applied before the rule partition

- Plan: silent on the kernel read caps.
- Code: `read_visible` keeps the newest `MAX_READ_ROWS` (8192) rows by
  `created_commit_seq` and cuts at `MAX_READ_ROW_BYTES`
  (`crates/daemon/src/kernel_routes/read.rs:26-29, 188-232`) before the
  daemon sees `source_kind`. A rule older than the newest 8,192 memory rows
  is absent from the candidate set, is not counted in `rules_omitted`, and
  only `truncated: true` hints at it.
- Why unsettled: whether "omission is disclosed, never silent" covers
  read-cap omission, and whether a rules-first read (a second
  `RowSelection` by `source_kind`, or a reordered selection) is in scope.
- Resolution evidence: a decision on scope; if in scope, a kernel-side
  selection by `source_kind` and a disclosure rule for `truncated`.
  Records: `packing-omitted-count-is-disclosed-reported-and-digested`,
  `packing-selection-never-admits-every-rendered-row-is-kernel-visible`.

### OQ-RS-4 Pi in-process child sessions and the directive command

- Plan: KTD8 documents `is_subagent: false` for Pi; U4 step 2 registers the
  command "only in the top-level entry (the subagent child entry registers
  tools only)".
- Code: `packages/pi-plugin/src/index.ts:332-340`: spawned children
  (`EIDNARA_PI_SUBAGENT=1`) return early; in-process `@gotgenes/pi-subagents`
  children "re-run this factory, so the spawned-child guard does not run";
  a process-global latch records the runtime.
- Why unsettled: whether an in-process child registers the command and
  whether its `ctx.ui.confirm` is a human dialog was not determined from the
  code read here.
- Resolution evidence: a read of the latch logic and a Pi-subagents
  fixture run. Records: `parity-directive-command-dispatch-is-host-user-only`,
  `parity-is-subagent-true-suppresses-m0-and-pi-sends-false`.

### OQ-RS-5 Directive mapping keyed on `source_kind` alone

- Plan: KTD6 maps `source_kind = user_directive` to `USER_DIRECTIVES`;
  KTD5 partitions required rows by `source_kind` alone.
- Code: the Kernel does not constrain `decision_kind` for a `source_kind`;
  a test seed can write (`CONSTRAINTS`, `user_directive`).
- Why unsettled: whether the design intends the pair (`adr_accepted`,
  `user_directive`) or `source_kind` alone. Production has one writer, so
  the difference is only visible to seeds and future writers.
- Resolution evidence: a one-line decision. Record:
  `packing-user-directive-renders-under-user-directives-by-source-kind`.

### OQ-RS-6 Pi version-drift policy for command dispatch

- Plan: KTD1 relies on the harness command surface being model-free.
- Code: pinned pi 0.80.2 hard-codes `expandPromptTemplates: false` in
  `sendUserMessage`; locally installed pi 1.3.0 accepts an option that
  enables command dispatch from an extension.
- Why unsettled: the pass cannot decide whether the confirm gate is
  considered sufficient under the newer behavior or whether the pin is a
  security boundary.
- Resolution evidence: an owner statement; a version-pinned test that fails
  on upgrade. Record: `parity-directive-command-dispatch-is-host-user-only`.

### OQ-RS-7 Pi `is_subagent` asymmetry: document or derive

- Plan: document (KTD8, U6 step 4). Plan OQ4 is the related blocking
  question for compaction-off parity.
- Code: Pi sends a constant `false` (`pi-pass-inputs.ts:110`); OpenCode
  sends the host value (`transform-pass-fields.ts:66`).
- Why unsettled: the plan's reason ("`parentSession` only at `newSession`")
  does not rule out a per-session map; this is a product decision about
  whether Pi children should see project rules.
- Resolution evidence: owner decision. Record:
  `parity-is-subagent-true-suppresses-m0-and-pi-sends-false`.

## Resolvable at implementation

### OQ-RS-8 Reuse of `admit_required` / `reserve_required`

- Plan: KTD5 charges required rows "through `admit_required` /
  `reserve_required`"; "the first `OverBudget` stops the phase".
- Code: `reserve_required` is all-or-nothing (`Err(OverBudget)` reserves
  nothing, `crates/retrieval/src/packing/required.rs:176-205`);
  `admit_required` consumes retrieval `SelectedOccurrence` facts
  (`:114-174`). Memory rows are `VisibleRow` plus `DecisionRow`.
- Resolution evidence: U2 either calls the primitive per prefix with an
  adapter or writes a memory-specific prefix loop sharing `TokenCount`; the
  prefix semantics of
  `packing-required-phase-is-a-prefix-ending-at-first-overbudget` hold
  either way. `crates/retrieval/AGENTS.md:66-68` (the plan cites `:71-74`;
  line drift) is updated to match.

### OQ-RS-9 Single oracle for Entity matching over an ancestor set

- Code: `ScopeMatchContext` holds one value per dimension
  (`crates/kernel/src/scope.rs:869-873`); `scope_matches` cannot take a set
  on the context side. The `ScopeFilter` docstring's "cannot disagree" claim
  (`project.rs:177-180`) is Project-only after U1.
- Resolution evidence: one daemon function that evaluates Project through
  `names_project` and Entity by ancestor iteration with the stated
  `Uncertain` rule, with a fixture table; the docstring narrowed.

### OQ-RS-10 Branch value form

- Plan: gix `head().referent_name()`.
- Resolution evidence: pin full ref name or short name in the context
  builder; the deferred `--branch` writer must use the same form.

### OQ-RS-11 `cwd` canonicalization and symlinks

- Plan: KTD7 "absolute path, same bounds as `project_root`"
  (`docs/host-wire-protocol.md:336`: 4,096 bytes, absolute platform path).
- Resolution evidence: decide whether `cwd` is canonicalized like the root
  before the prefix comparison; test both symlink directions.

### OQ-RS-12 Disclosure line reservation when no rule exists, and its cost basis

- Plan: "reserved before packing so it is never a victim".
- Resolution evidence: pin whether `d` is reserved unconditionally and
  whether it is estimated from a fixed-width bound or from the actual `N`.

### OQ-RS-13 Digest version string

- Code: `"eidnara-project-memory-revision-v1"` (`canonical_memory.rs:57`).
- Resolution evidence: pin whether `N` is hashed unconditionally (every
  revision changes on upgrade) or only when positive.

### OQ-RS-14 Minimum-service bound when a second rule is omitted

- Plan: "the first rule is served when it fits `budget - wrapper`".
- Resolution evidence: pin whether the bound is `budget - wrapper -
  disclosure` when `n > 1`, so the disclosure line always fits, or the
  plan's bound with an accepted overrun of `d` tokens in that band.

### OQ-RS-15 Specificity under `Set` Entity terms

- Plan: "count of non-Project terms matched, then `dir:` depth".
- Resolution evidence: pin depth as the matched member's depth or the
  maximum member depth for `Set` terms.

### OQ-RS-16 Parity domain is the derived CK window

- Code: 7.10.3 native derivation differs per codec; existing records
  `codec-b-opencode-hides-four-part-types-from-every-transform-decision`
  and `codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record`.
- Resolution evidence: state R13's "pass context" over the derived window
  plus `cwd`; choose e2e tool-call shapes both codecs preserve.

### OQ-RS-17 `tool_present` after the Pi registration split

- Code: `index.ts:383` ties reduce-tool registration to `compactionOff`;
  `:593` sends `reduceRegistered: true` unconditionally.
- Resolution evidence: after the split, send the actual registration state.

### OQ-RS-18 History-summarizer read context (plan OQ7)

- Plan: `cwd`-only context for `lib.rs:6006`.
- Resolution evidence: implement and test that the summarizer read reports
  its own `context_resolved` or none, and that it never serves a `dir:` rule
  by window paths.

### OQ-RS-19 Is the disclosure line provider-visible?

- Unknown whether any surface strip touches `<project-memory>` children
  (`render-a-hygiene-metric-ignores-surface-strips` names the class).
- Resolution evidence: trace m0 bytes through the strips; compare parity on
  daemon m0 bytes (`extractM0`) regardless.

### OQ-RS-20 7.8 method-literal count

- Code: `docs/host-wire-protocol.md:739` says "eleven method literals".
- Resolution evidence: U3 updates the count to fifteen with the four
  `directive.*` literals (plan U3 step 7 already names this).
