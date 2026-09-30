# fa-e02-raw-mode-controls-host-surfaces

## Discovery trigger

Existing-behavior record FA-E02 of #834's first comment, at `265df096`.
Surfaces: plugin and Pi. PR #898 (#852) replaces the "raw boolean only"
clause with the chain clause of D1 (FA-N01), and its merged history also
withdraws folding on an unresolved admission. PR #898's description records
the replacement; its checks were updated rather than deleted.

Exercised status: yes - for the surviving clauses. The predicate test and the
tool-registry and command-handler tests ran in the `bun run check:repo` gates
of #898 through #904 and in #859 PR A's at `d7b330113`. The TypeScript
source at HEAD is `d7b330113`'s: after #904's head it carries the review
changes merged through `main` (fold-authority warning delivery and
reconciliation), and `d7b330113` itself changes only comments and generated
TUI source (`plugin/rpc-handlers.ts`, `tui/index.tsx`,
`tui-compiled/index.tsx`).

## Evidence trail

At `265df096`: `isCompactionEnabled` returned
`config.compaction?.enabled !== false`
(`packages/opencode-plugin/src/config/agent-disable.ts:19-23` at that tree).

At HEAD `0ff62b29a`:

- `isCompactionEnabled` (`packages/opencode-plugin/src/config/agent-disable.ts:21-29`)
  returns `compaction?.enabled !== false` and
  `normalizeSummarizerChain(history_summarizer).length > 0`. It changed only
  in `bd592e4a0` (#898); `git log` shows no later commit to the file.
- Unresolved admission forces `compaction.enabled = false` with a warning:
  `withdrawUnresolvedFoldAuthority`
  (`packages/opencode-plugin/src/config/fold-authority.ts:137-146`), from
  `f5b10fefe`, which is in #898's merged history (`git merge-base
  --is-ancestor f5b10fefe dd5dcc170`).
- Tool registry: `compactionOff = !isCompactionEnabled(pluginConfig)` omits
  `eidnara_reduce` (`packages/opencode-plugin/src/plugin/tool-registry.ts:38-48`;
  removed set at `:19`).
- Hook: `compactionOff` is resolved once (`hooks/context/hook.ts:226`) and
  threaded to the phases (`:531`, `:545`, `:605`).
- Commands: `getEidnaraBuiltinCommands(isCompactionEnabled(pluginConfig))`
  (`packages/opencode-plugin/src/index.ts:310`) keeps every command registered
  and swaps the description of recomp, wrapup, and flush for an
  "Unavailable" text (`features/builtin-commands/commands.ts:6-31`). The
  handler refuses `/eidnara-flush`, `/eidnara-recomp`, and `/eidnara-wrapup`
  with a notification and no daemon call
  (`hooks/context/command-handler.ts:505-513`).
- Pi: `compactionRequested = isCompactionEnabled(config)` and Pi's own
  transform-availability gate decide `compactionOff`
  (`packages/pi-plugin/src/index.ts:349-356`).

Checks at HEAD:

- `isCompactionEnabled requires a summarizer chain and a compaction setting that is not false`
  (`packages/opencode-plugin/src/config/compaction-accessor-guard.test.ts:67`).
- `compaction-off tool set = mode-on tool set minus exactly the reduce factory's IDs, ...`
  and `an empty summarizer chain registers the compaction-off tool set`
  (`packages/opencode-plugin/src/plugin/tool-registry.test.ts:127`, `:154`).
- `refuses /${command} without a daemon call when compaction is off`
  (`packages/opencode-plugin/src/hooks/context/command-handler.test.ts:144`,
  inside the loop at `:143`).

Citation corrections from `265df096` to HEAD: `agent-disable.ts:19-23` is now
`:21-29`; `tool-registry.ts:34-48` is `:38-48`; `hook.ts:223` is `:226`;
`index.ts` (Pi) `:84,349-355` is `:13,349-356`;
`compaction-accessor-guard.test.ts:66` is `:67`.

## Failure scenario

Before #898, a user with no summarizer model and no explicit `compaction`
setting ran in "compaction on" mode: `eidnara_reduce` registered and fold
commands accepted, while nothing could fold.

## Timing windows and dependencies

None; the mode is resolved once at plugin startup.

## What a test must construct

An empty configuration, an explicit `false`, a model with each compaction
setting, and a blank model; the registered tool set and the command refusal
for an empty chain.

## Investigation log

### Q: Do the refusal and command texts name the right cause under the chain clause?

- Sources examined: `command-handler.ts:505-513`; `commands.ts:7-8`;
  the refusal test at `command-handler.test.ts:144-160`.
- Findings: No. Both texts say `compaction.enabled` is false. With an empty
  chain the setting may be absent or true, so the text names the wrong cause.
  The test pins the old text.
- Missing evidence: The owner's intended wording.
- Conclusion: needs human input (contract-versus-code disagreement in user
  text; the behavior is correct).

### Q: Is the TUI comment on config-load failure still true?

- Sources examined: `packages/opencode-plugin/src/tui/index.tsx:1097-1105`;
  `git show d7b330113`.
- Findings: No, at `286ffaf81`. The comment at `:1098` there said
  `isCompactionEnabled` defaults to `true` when the config fails to load, but
  `isCompactionEnabled({})` is `false` (the predicate test asserts it). The
  code passes `pluginConfig ?? {}`. `d7b330113` removed the comment.
- Missing evidence: None.
- Conclusion: resolved with answer; the comment was removed at `d7b330113`.

### Q: Did #900 change the predicate?

- Sources examined: `git log` of `agent-disable.ts` and `fold-authority.ts`;
  PR #900 description.
- Findings: No. #900 derives setup's and doctor's authority through
  `foldAuthorityOf`, which restates the rule over the same
  `normalizeSummarizerChain` and adds the `enabled: false` and unresolved
  arms (`packages/opencode-plugin/src/config/fold-authority.ts:153-171`).
  #904 (`a4827a892`) moved `FoldAuthority`, `foldAuthorityOf`, and
  `describeFoldAuthority` there from the CLI. The rule therefore has two
  expressions, `isCompactionEnabled` and `foldAuthorityOf`; they agree on
  the chain and `compaction.enabled` arms by reading.
- Missing evidence: None.
- Conclusion: resolved with answer.
