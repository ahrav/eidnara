# fa-n07-conflict-disposition-is-authoritative

## Discovery trigger

Proposed record FA-N07 of #834 (D2a, the `native_compaction_active` clause of
D2c; C1). Ticket #853 implements it; PR #899 lands it. PR #904 (#856) adds the
pending-authority and configuration-root-mismatch warnings through the same
disposition path. Surfaces: plugin, cli.

Exercised status: partial - detector, fixer, status RPC, TUI label, and
entry-bundle boot tests ran green in #899's and #904's `bun run check:repo`
gates. TUI startup under `warn` has no runtime witness.

## Evidence trail

Package references are verified at `d7b330113`, the package tree of #859
PR C's head; no file under `packages/` changes after it. Between #904 (`57a820bb6`) and `d7b330113` the package tree takes the
review changes merged through `main` (fold-authority warning delivery and
reconciliation); `d7b330113` itself changes only comments and generated TUI
source, and package contents are equal across the three #859 slices.

- Detection: `detectConflicts`
  (`packages/opencode-plugin/src/shared/conflict-detector.ts:88-161`) sets
  `noFoldAuthority` and targets `auto = true` when Eidnara does not fold and
  the resolved `auto` is false (`:117-121`); under Eidnara folds `auto` or
  `prune` true set `compactionAuto`/`compactionPrune` with targets `false`
  (`:107-116`). DCP and the three OMO hooks are flagged in both modes
  (`:127-151`).
- Disposition: `conflictDisposition` (`:166-170`) returns `disable` when any
  flag other than `noFoldAuthority` is set, else `warn` when that flag is set,
  else `none`. The actionable patch holds a target key only when the config
  files control it; a key set by `OPENCODE_DISABLE_AUTOCOMPACT`, inline
  `OPENCODE_CONFIG_CONTENT`, or a host layer outside the config files goes to
  `unresolved` with its source, so native folds with an overridden
  `auto = false` give `warn`, an empty patch, and one unresolved entry
  (`splitCompactionPatch`, `:172-195`; `compactionOverrideSource`,
  `:197-204`; tests `conflict-detector.test.ts:578-623`).
- Consumers: server boot branches on the disposition and clears
  `pluginConfig.enabled` only on `disable`
  (`packages/opencode-plugin/src/index.ts:144-155`);
  the Desktop warning uses it (`index.ts:217-225`;
  `plugin/conflict-warning-hook.ts:290`);
  TUI startup shows the dialog for any non-`none` result and returns only on
  `disable` (`tui/index.tsx:1106-1116`, dialog wording `:86`); setup
  (`packages/cli/src/commands/setup-opencode.ts:663-672`, `:422-440`); doctor
  (`packages/cli/src/commands/doctor-opencode.ts:549-599`); diagnostics
  (`packages/cli/src/lib/diagnostics-opencode.ts:542-545`); formatting
  (`formatConflictShort`, `conflict-detector.ts:607-621`, headers `:602-603`).
- Repair: `fixConflicts` (`packages/opencode-plugin/src/shared/conflict-fixer.ts:110`)
  writes the patch's target values and compares against the host defaults
  (`HOST_COMPACTION_DEFAULTS`, `:80`; `:102`); it reports "Enabled
  auto-compaction" (`:205`).
- Observation: `registerRpcHandlers` sets `nativeActive` from the resolved
  `compaction.auto` alone
  (`packages/opencode-plugin/src/plugin/rpc-handlers.ts:790-791`)
  and omits `native_compaction_active` when it is undefined (`:543-545`). The
  TUI label renders absence as "compaction owner unknown"
  (`tui/compaction-off.ts:9-12`).
- #904 warnings: `reportFoldAuthority` publishes a `ConflictWarning` with
  disposition `warn` for a pending target or a root mismatch, and publishes
  `undefined` once the session's status clears
  (`shared/fold-authority-status.ts:247-260`); the witness is
  "publishes a warn conflict that formats under the warning header, and a
  clear state" (`shared/fold-authority-status.test.ts:282`). Since the
  later #904 commits on `main` (`1b69348d4` through `b6a9d6605`),
  `publishOnChange` (`:85-101`) forwards a session's first state and each
  change, and forwards an unchanged state again after a delivery that failed
  or threw; the conflict-warning hook reconciles queued warnings in poll
  order.

Gate results of the runs cited, as recorded:

- #899 (base `a0f39d7f5`, merged head `566a35c10`, merge `f653cecc4`):
  `bun run check:repo`, smoke, comment markers pass.
- #904 (branch head `57a820bb6`): `bun run check:repo`, smoke, markers, fmt,
  clippy pass; `cargo test --workspace` pass except one flaky untouched
  host-runtime test, green on rerun.

## Failure scenario

A `warn` disables the plugin, its RPC server, or the sidebar, so a user with
no summarizer loses memory features over a host setting. Or a `prune = true`
or absent observation reports native compaction as active when nothing folds.

## Timing windows and dependencies

Boot reads the host's resolved configuration through `resolveCompactionForBoot`;
if that fetch fails the file-based check runs (`index.ts:137-147`). The TUI
re-detects against the files after a repair (`tui/index.tsx:1111-1114`).

## What a test must construct

The mode-by-host matrix; `warn` plus DCP and plus OMO; environment, inline,
and host-only `auto = false`; both RPCs with `auto = false, prune = true`,
`auto = true`, and no observation; a boot with no fold authority asserting the
RPC server and sidebar stay up; the TUI startup branch under `warn`.

## Investigation log

### Q: Is the TUI `warn` branch exercised at runtime?

- Sources examined: `tui/index.tsx:1095-1118`; #899's description; the
  `tui/*.test.ts` files.
- Findings: No. #899 records that TUI startup has no unit harness and that
  typecheck, the compiled-TUI freshness check, and the smoke import cover it.
  The entry-bundle test asserts the RPC port directory, not the sidebar.
- Missing evidence: A TUI startup harness with a stubbed `api`.
- Conclusion: unresolved, needs a TUI startup witness.

### Q: How does the TUI read a configuration-load failure?

- Sources examined: `tui/index.tsx:1097-1106`; `config/agent-disable.ts:21-29`;
  `index.ts:77`.
- Findings: The TUI catches the load error and passes `{}` to
  `isCompactionEnabled`, which is false at HEAD because the chain is empty, so
  the TUI detects under native folds. At `286ffaf81` the comment at
  `tui/index.tsx:1098` said the call "defaults to `true`", which did not
  hold; `d7b330113` removed it. The server does not catch the error
  (`index.ts:77`), so it refuses to start. A second comment, at
  `plugin/rpc-handlers.ts:386-387`, said `nativeActive` reads `auto` or
  `prune`, while the code reads `auto` alone; `d7b330113` corrected it to
  `compaction.auto`.
- Missing evidence: The intended TUI reading of a load failure.
- Conclusion: needs human input.

### Q: Do #904's later commits on `main` change the disposition or its consumers?

- Sources examined: `git log 1b746e78b..d7b330113 -- packages` (the plugin
  commits `1b69348d4` through `b6a9d6605`, and `0023302db`);
  `packages/opencode-plugin/src/shared/fold-authority-status.ts:85-101`,
  `:247-260`; `packages/opencode-plugin/src/shared/conflict-detector.ts:166-170`.
- Findings: No. `detectConflicts`, `conflictDisposition`, and the boot,
  TUI, setup, and doctor branches are unchanged. The fold-authority
  warnings are now published per session: `reportFoldAuthority` publishes a
  `warn` state or a clear state, `publishOnChange` forwards only changes and
  retries a delivery that failed, and the conflict-warning hook reconciles
  queued warnings in poll order. The witness test was renamed to "publishes
  a warn conflict that formats under the warning header, and a clear state".
- Missing evidence: None.
- Conclusion: resolved with answer: evidence updated; the record stays
  `active`.
