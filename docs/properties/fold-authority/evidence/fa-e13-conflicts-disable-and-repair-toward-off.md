# fa-e13-conflicts-disable-and-repair-toward-off

## Discovery trigger

Existing-behavior record FA-E13 of #834 (first comment), describing the
plugin's conflict handling at `265df096`. D2a replaces it with a derived
disposition and target-value repair (PR #899, #853); D2b replaces doctor's
repair direction (PR #900, #854). FA-N07 and FA-N08 are the replacing
obligations. Surfaces: plugin, cli.

Exercised status: partial - the surviving clauses and the replacements have
named tests that ran in #899's and #904's `bun run check:repo` gates (pass);
the TUI startup branch has no runtime witness (see the question below).

## Evidence trail

Package references are verified at `d7b330113`, the package tree of #859
PR C's head; no file under `packages/` changes after it. Between #904 (`57a820bb6`) and `d7b330113` the package tree takes the
review changes merged through `main` (fold-authority warning delivery and
reconciliation); `d7b330113` itself changes only comments and generated TUI
source, and package contents are equal across the three #859 slices.

Clauses and their disposition at HEAD:

- "Any detected conflict disables server participation": replaced. Boot
  clears `pluginConfig.enabled` only on `disable` and logs and continues on
  `warn` (`packages/opencode-plugin/src/index.ts:144-155`). The old test
  was `hasConflict` (`index.ts:111-113` at `265df096`); `hasConflict` no
  longer exists.
- "Stops TUI initialization": replaced. The TUI shows the dialog for any
  non-`none` result and returns only on `disable`
  (`packages/opencode-plugin/src/tui/index.tsx:1110-1116`); at `265df096` it
  returned on any conflict (`:1055-1058`).
- "Compaction repair writes only `false`": replaced. `fixConflicts`
  (`packages/opencode-plugin/src/shared/conflict-fixer.ts:110`) writes each
  patched key's target value (`:120-145`) into the layer
  `compactionRepairTarget` picks (`:89-104`), comparing a key no layer sets
  with the host default (`HOST_COMPACTION_DEFAULTS`, `:80`; `:102`) instead
  of `false`. At `265df096` the writes were literal `false`
  (`conflict-fixer.ts:146`, `:156`). Doctor's direction follows the patch,
  including `auto = true` under native folds
  (`packages/cli/src/commands/doctor-opencode.ts:571-574`).
- "The environment flag forces `auto` false in the file-based fallback":
  preserved. `checkCompaction` applies `OPENCODE_DISABLE_AUTOCOMPACT` after
  the layer merge (`packages/opencode-plugin/src/shared/conflict-detector.ts:350-364`,
  flag at `:361`; the same return was `:295-297` at `265df096`). A forced key is now reported unresolved by source and left out of
  the patch (`splitCompactionPatch`, `:172-195`; `compactionOverrideSource`,
  `:197-204`).
- "Which a supplied resolved configuration bypasses": preserved. Detection
  uses `options.resolvedCompaction` when supplied (`:106`; `:94` at
  `265df096`).

Spec citations at `265df096` that moved: `conflict-detector.ts:79-143`
(`detectConflicts`) is now `:88-161`; `conflict-fixer.ts:87-100` is
`:89-104`; `:128-159` is `:118-145`. The existing checks:
`conflict-detector.test.ts:498-558` (the compaction-off matrix) is now the
`describe` at `:480` with its row table at `:502` and the mixed rows at
`:558`; `conflict-fixer.test.ts:248` ("leaves the user config untouched when
the project layer set auto=true") is now `:290`; `:598-672` (the
compaction-off parity `describe`) is now `:705` onward.

Witnesses at HEAD:

- File-based arm: `conflict-detector.test.ts:1043` ("OPENCODE_DISABLE_AUTOCOMPACT
  host semantics (file-based arm)", rows `:1064`, `:1079`).
- Resolved arm: "OPENCODE_DISABLE_AUTOCOMPACT does not override the resolved
  arm (host already applied it)" (`:743`).
- Warn keeps the plugin: `packages/opencode-plugin/src/index.entry.test.ts:203`.
- Target-value repair: `conflict-fixer.test.ts:659`, `:679`, `:689`; doctor
  `packages/cli/src/commands/doctor-opencode.test.ts:359`, `:398`.

Gate results of the runs cited:

- #899 (merged head `566a35c10`): `bun run check:repo`, smoke, markers pass.
- #900 (merged head `2effd440f`): `bun run check:repo`, smoke, markers pass.
- #904 (branch head `57a820bb6`): `bun run check:repo` pass.

## Failure scenario

A surviving clause regresses: the file-based arm ignores the flag and
reports a repairable `auto = true` the host never applies, or the resolved
arm re-applies the flag and hides a real conflict.

## Timing windows and dependencies

Boot falls back to the file-based arm only when the resolved-config fetch
fails (`index.ts:137-143`).

## What a test must construct

Each disposition at boot and in the TUI; a repair under both authorities; the
flag with the file-based arm and with a supplied resolved configuration.

## Investigation log

### Q: Is FA-E13 replaced or preserved?

- Sources examined: #834 D2a, D2b; #899's and #900's "Replaced property
  clauses"; the code above.
- Findings: Replaced in part. The disable-on-any-conflict and false-only
  repair clauses are gone. The environment flag's effect on the file-based
  arm and its bypass by a supplied resolved configuration survive; the flag
  is now also reported unresolved by source.
- Missing evidence: None.
- Conclusion: resolved with answer; invalidated with the surviving clauses
  listed.

### Q: Is the TUI boot branch witnessed?

- Sources examined: `tui/index.tsx:1106-1116`; #899's description.
- Findings: No runtime witness; see FA-N07.
- Missing evidence: A TUI startup harness.
- Conclusion: unresolved, needs a TUI startup witness.
