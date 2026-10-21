# Fault map: proposed read side (N3, N4, N5)

Every entry describes a fault or enabling state a record needs, whether it
is constructible at an existing seam at HEAD, and the record(s) it serves.
"Seam" names the existing injection point; "none" means a new fixture or
hook is required. Nothing here is implemented; the map is a plan for the
campaign once U1, U2, U4, and U6 land.

## Fault classes and availability

| Fault or state | Constructible at HEAD | Seam | Records |
| --- | --- | --- | --- |
| Detached HEAD on the bound project | yes | temporary git repo, `git checkout --detach` (U1 tests, `crates/daemon/tests/kernel_routes.rs` style) | scope-context-is-resolved-per-pass-from-named-sources, scope-rule-serves-only-when-every-carried-term-matches |
| Unborn HEAD or non-git project | yes | empty directory as `project_root` | scope-context-is-resolved-per-pass-from-named-sources |
| gix error (corrupt `.git`) | yes | truncate `.git/HEAD` | scope-context-is-resolved-per-pass-from-named-sources |
| Environment key unset / set | yes | config tiers (`crates/daemon/src/config.rs:1198-1201` pattern) | scope-context..., scope-rule-serves-only... |
| `cwd` absent, outside root, symlinked, relative | partly | transform request builder in tests; the `cwd` field does not exist yet | scope-directory-ancestors-derive-only-from-paths-inside-project-root |
| Tool-call input with non-string, nested, or oversized path members | yes | CK window fixtures (`crates/daemon/tests/fixtures/typed-wire-decode/*`) | scope-directory-ancestors... |
| Path only in a tool result or text block | yes | same fixtures | scope-directory-ancestors... |
| Rows with Branch / Environment / Entity terms | yes | kernel scope term seeding via commit routes (`ScopeTermSpec`) | scope-rule-serves-only... |
| Row with a `RedactedPlaceholder` term | yes | kernel scope algebra fixtures (`crates/kernel/tests/kernel_scope_algebra.rs:193`) | scope-rule-serves-only... |
| Row with no Project term | yes | direct kernel seeding | scope-rule-serves-only... |
| Hidden row with `source_kind = user_directive` | test-only | kernel seeding with model classes (bypasses the directive route) | packing-selection-never-admits... |
| `review_only` labeled row | yes | existing memory reviewer fixtures | packing-selection-never-admits... |
| `adr_accepted` row with `source_kind != user_directive` | test-only | kernel seeding | packing-user-directive-renders... |
| Equal `created_commit_seq` across rules | yes | one envelope inserting several decisions | packing-required-order... |
| HashMap insertion-order shuffle of `decisions` | yes | construct `ReadResponse` directly (`canonical_memory.rs:265` helper) | packing-required-order... |
| Budget of 0, NaN, `w-1`, `w+d-1`, `w+d` | yes | `memory_budget_tokens` f64 input | packing-reserve-is-computed-once... |
| Reserve > budget, reserve 0 | partly | config key does not exist yet | packing-reserve... |
| Nine rules exceeding the reserve with a small rule after an oversize one | yes | seeded rows with controlled text lengths | packing-required-phase-is-a-prefix..., packing-omitted-count... |
| First rule between reserve and `budget - wrapper`; first rule over the whole budget | yes | seeded row of ~2,400 and ~5,000 tokens | packing-first-rule-is-served... |
| Rule over 64 KiB | partly | `memory-seed` control frame is capped at 64 KiB (`cf-memory-credit...` evidence) | packing-first-rule-is-served... |
| Omitted count change with the same served rows | yes | add rules beyond the limit between two snapshot constructions | packing-omitted-count... |
| Process crash between kernel writes | n/a on read side | N1/N2 seams | none here; the read side observes a committed snapshot |
| Concurrent passes, two sessions, one project | yes | two `CaseDriver` sessions | parity-identical-request-fields... |
| Commit between two parity passes | yes | `directive.record` (N1) or `memorySeed` between passes | parity-identical-request-fields... (negative) |
| Replayed `operation_key` on the directive command | n/a here | N1 route | parity-directive-command-writes-only... (caller action) |
| Model tool call with directive-shaped text | yes | `eidnara_memory create` in e2e | parity-directive-command-dispatch-is-host-user-only |
| `pi.sendUserMessage("/eidnara-directive ...")` | yes | a test extension in the Pi harness | parity-directive-command-dispatch... |
| Queued steer/follow-up naming the command | yes | Pi harness while streaming | parity-directive-command-dispatch... |
| `EIDNARA_PI_SUBAGENT=1` child | yes | env var in the Pi fixture | parity-directive-command-dispatch... |
| In-process `@gotgenes/pi-subagents` child | unknown | needs the subagent extension in the fixture | parity-directive-command-dispatch... (open) |
| Pi `ctx.hasUI == false` | yes | headless Pi fixture | parity-directive-command-writes-only... |
| OpenCode command without a dialog | yes (it is the only state) | OpenCode fixture | parity-directive-command-writes-only... (open) |
| `--yes`-style token in arguments | yes | runner unit test | parity-directive-command-writes-only... |
| Session replaced between confirm and commit | yes | `isCancelled` injection in runner tests | parity-directive-command-writes-only... |
| Pi compaction off | yes | Pi config `compaction.enabled: false` | parity-pi-context-handler-registers... |
| `/cd` to a project with different compaction config | yes | Pi fixture | parity-pi-context-handler-registers... (documented non-re-registration) |
| `is_subagent: true` pass | yes | request field | parity-is-subagent-true-suppresses-m0... |
| Pi version drift (1.3.0 `sendUserMessage` expansion) | yes locally, not in the pinned tree | upgrade the pin in a scratch worktree | parity-directive-command-dispatch... |
| Codec asymmetry on tool-call part types | yes | native window fixtures per profile | parity-identical-request-fields... |
| Read cap (`MAX_READ_ROWS` 8192) exceeded with old rules | yes but slow | seed 8,193 memory rows | open question on read-cap interaction (no record) |

## Per-record required faults

- scope-context-is-resolved-per-pass-from-named-sources: detached HEAD
  between passes; environment key toggled; `cwd` changed between passes.
- scope-rule-serves-only-when-every-carried-term-matches: rows on four
  dimensions; contexts with each dimension absent; a Project-less row; a
  redacted term.
- scope-directory-ancestors-derive-only-from-paths-inside-project-root:
  outside path; `..` path; non-string member; nested member; path only in a
  result block; symlinked `cwd`.
- packing-selection-never-admits-every-rendered-row-is-kernel-visible:
  Hidden `user_directive` row; labeled row; directive-shaped model row.
- packing-required-order-is-total-and-identical-across-passes: equal seq;
  equal specificity across channels; shuffled map order.
- packing-reserve-is-computed-once-after-wrapper-and-disclosure: boundary
  budgets; reserve extremes.
- packing-required-phase-is-a-prefix-ending-at-first-overbudget: the nine-
  rule cost vector; `[1500, 200, 100]`.
- packing-first-rule-is-served-when-it-fits-budget-minus-wrapper: single
  rules at 2,400 and 5,000 tokens; `[2400, 2400]`; `[3990, 100]`.
- packing-omitted-count-is-disclosed-reported-and-digested: N from 0 to 4
  with fixed served rows; grammar fuzz on the rendered block.
- packing-user-directive-renders-under-user-directives-by-source-kind: the
  four (`source_kind`, `decision_kind`) pairs.
- parity-directive-command-dispatch-is-host-user-only: model tool call;
  `sendUserMessage`; queued text; both child kinds.
- parity-directive-command-writes-only-after-ui-confirmation: no UI; declined;
  cancelled; flag token.
- parity-identical-request-fields-yield-identical-rule-sets: same snapshot
  both harnesses; omitted `cwd` on one; pressure variant.
- parity-pi-context-handler-registers-whenever-project-binds: compaction off
  with a seeded rule.
- parity-is-subagent-true-suppresses-m0-and-pi-sends-false: `true` pass vs
  `false` pass.

## Coverage checks to add

Markers assert independent preconditions, never the violation:

- `scope_context_branch_absent`: a pass ran with `context_resolved.branch ==
  null` while at least one Branch-term rule existed in the snapshot.
- `scope_context_entity_empty`: a pass ran with `paths == []` or `["."]`
  while at least one `dir:` rule deeper than `.` existed.
- `packing_required_overbudget_reached`: the required phase ended with at
  least one unserved required candidate.
- `packing_minimum_service_band`: the first required row's cost exceeded
  the required limit and fit `budget - wrapper`.
- `packing_channel_outranked_recency`: a `user_directive` row older than a
  served `repository_policy` row was served first.
- `parity_negative_control_diverged`: the two harness passes differed in
  `context_resolved.cwd`.
- `directive_command_no_ui`: the runner was invoked with no confirm
  callback.
- `pi_pass_compaction_off`: a Pi pass body was sent while compaction was
  off.

## Ranking by cheapest valid oracle

1. Pure-function tables (ancestor derivation, order comparator, budget
   plan, prefix packer, category mapping): cheapest; cover eight records
   without a daemon or harness.
2. `ReadResponse`-level snapshot tests (existing helper at
   `canonical_memory.rs:223-280`): visibility, mapping, digest, disclosure
   grammar.
3. Daemon integration with a temporary git repo and seeded rows: scope
   context across HEAD states; `is_subagent` suppression.
4. Harness unit tests on runners and field builders: confirm gate, actor
   literals, `is_subagent` constant, `cwd` presence.
5. E2E across both harnesses (`native-addon` job): parity, compaction-off
   Pi, AE1/AE2/AE8. Most expensive; run last.
