# fa-e12-cli-mode-resolution-does-not-use-the-plugin-loader

## Discovery trigger

Existing-behavior record FA-E12 of #834 (first comment), describing the CLI
at `265df096`. D1 and D1a replace its mode resolution (PR #898, #852); D2
replaces its native setup clause (PR #900, #854). FA-N01 and FA-N08 are the
replacing obligations. Surface: cli.

Exercised status: yes - the surviving clause and both replacements have
named tests that ran in #904's `bun run check:repo` gate (pass).

## Evidence trail

Package references are verified at `d7b330113`, the package tree of #859
PR C's head; no file under `packages/` changes after it. Between #904 (`57a820bb6`) and `d7b330113` the package tree takes the
review changes merged through `main` (fold-authority warning delivery and
reconciliation); `d7b330113` itself changes only comments and generated TUI
source, and package contents are equal across the three #859 slices.

Clauses and their disposition at HEAD:

- "CLI mode resolution reads raw lenient JSONC": replaced.
  `readEidnaraModes` (`packages/cli/src/lib/eidnara-modes.ts:17-33`) calls
  `loadUserTierConfigDetailed` (`:21`), the plugin's user-tier pipeline
  (`packages/opencode-plugin/src/config/index.ts:638-642`), and returns its
  `admission`. Raw lenient JSONC remains for editing and for the project-tier
  disagreement report (`projectModeOverrides`, `eidnara-modes.ts:39-51`).
- "Forwards only the compaction block": replaced. `compactionEnabledFor`
  (`:54-64`) forwards `compaction` and `history_summarizer` to
  `isCompactionEnabled`; `enabled: false` still answers `false`.
- "Off-mode setup leaves native compaction untouched": replaced for the
  native path. Native folds write `NATIVE_FOLDS_COMPACTION = { auto: true }`
  (`packages/cli/src/commands/setup-opencode.ts:267`) and leave `prune` as
  found; the writer sets only the patch's keys
  (`addPluginToOpenCodeConfig`, `:87`; new file `:112-114`, existing file
  `:163-175`). An empty
  patch still leaves compaction untouched: setup passes `{}` when Eidnara is
  disabled or a repair is declined (`:652-657`, `:693`).
- "On-mode setup writes `auto` and `prune` false": preserved
  (`EIDNARA_FOLDS_COMPACTION`, `:266`).
- Setup's authority no longer comes from `readEidnaraModes`: it comes from
  the proposed document (`:603-613`; FA-N08). `modes` still gates enablement
  and OMO reach (`:65-66`, `:630`).

Spec citations at `265df096` that moved: `lib/eidnara-modes.ts:15-48` is now
`:17-64`; `commands/setup-opencode.ts:99-101,150-168` (the new-file and
existing-file compaction writes) is now `:112-114` and `:163-175`. The existing checks: `lib/eidnara-modes.test.ts:21` ("derives
every mode from the shared config and defaults to enabled") is now `:24`,
renamed "derives every mode from the shared config through the plugin
loader", and asserts `compactionEnabled: false` for a missing file;
`commands/setup-opencode.test.ts:632` and `:651` are now `:670` and `:689`
with the same names, driving the writer with an explicit patch.

Witnesses at HEAD:

- `packages/cli/src/lib/eidnara-modes.test.ts`: `:24`, `:53` ("forwards the
  summarizer chain after substitution and reference exclusion"), `:68`
  ("reports a rejected user tier as unresolved"), `:121` ("resolves the
  compaction mode setup produces once it writes the summarizer model").
- `packages/cli/src/commands/setup-opencode.test.ts:670` and `:689`.
- `packages/cli/src/commands/setup-opencode-authority.test.ts:136` (fresh
  native setup writes `{ auto: true }`), `:165` (model setup writes both
  false), `:221` (native repair keeps `prune: true`).

Gate results of the runs cited:

- #898 (merged head `4c964f93a`): `bun run check:repo` pass.
- #900 (merged head `2effd440f`): `bun run check:repo`, smoke, markers pass.
- #904 (branch head `57a820bb6`): `bun run check:repo` pass.

## Failure scenario

The CLI resolves native folds for a configuration the plugin reads as
Eidnara folds, so setup leaves OpenCode's compaction on beside the
summarizer; or native setup leaves a stale `auto = false`, and nothing folds.

## Timing windows and dependencies

None within one run; the resolution reads the files once.

## What a test must construct

A user tier whose chain the raw reader misreads (a reference, a module
model, a string fallback); a native setup over a prior `auto = false`; an
Eidnara-folds setup over `auto = true, prune = true`.

## Investigation log

### Q: Is FA-E12 replaced or preserved?

- Sources examined: #834 D1, D2; #898's and #900's "Replaced property
  clauses"; the code above.
- Findings: Replaced in part. #898 replaced the raw read and the
  compaction-only forwarding; #900 replaced the native setup clause. The
  Eidnara-folds write survives.
- Missing evidence: None.
- Conclusion: resolved with answer; invalidated with the surviving clause
  listed.

### Q: Does the CLI resolver run the parity fixture?

- Sources examined: `eidnara-modes.ts`; `eidnara-modes.test.ts`.
- Findings: No. It reads the user tier alone by design, so project-tier
  fixture rows do not apply. Its tests cover substitution outside the chain,
  reference exclusion, and unresolved admission.
- Missing evidence: None for the stated contract.
- Conclusion: resolved with answer.
