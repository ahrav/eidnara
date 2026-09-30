# fa-e11-typescript-expands-user-config-and-rejects-module-keys

## Discovery trigger

Existing-behavior record FA-E11 of #834 (first comment), describing the
OpenCode plugin loader at `265df096`. #834 D1a makes TypeScript the reference
loader and changes two of its clauses; PR #898 (#852) lands the change and
names FA-E11 "replaced". FA-N01 is the replacing obligation. Surfaces:
plugin, pi.

Exercised status: yes - each surviving clause has a named test, and the
replaced clauses are pinned by the parity fixture; all ran in #904's
`bun run check:repo` gate (pass).

## Evidence trail

Package references are verified at `d7b330113`, the package tree of #859
PR C's head; no file under `packages/` changes after it. Between #904 (`57a820bb6`) and `d7b330113` the package tree takes the
review changes merged through `main` (fold-authority warning delivery and
reconciliation); `d7b330113` itself changes only comments and generated TUI
source, and package contents are equal across the three #859 slices.

Clauses and their disposition at HEAD:

- "Discovers `.jsonc` before `.json`": preserved. `detectConfigFile`
  (`packages/opencode-plugin/src/shared/jsonc-parser.ts:227-243`); the user
  path helper (`config/config-paths.ts:40-44`).
- "Expands user-tier variables before parsing": preserved outside the chain
  keys, replaced inside them. `loadConfigTextDetailed`
  (`config/index.ts:131-195`) runs `screenUserTier` on the raw user text
  (`:137-140`) before `substituteConfigVariables` (`:141-145`;
  `config/variable.ts:106`). The screen blanks every chain literal that is
  blank or holds `{env:`/`{file:` at its token span, so it is never
  substituted or read (`config/fold-authority.ts:79-121`).
- "Accepts string-or-array fallbacks": preserved
  (`config/schema/agent-overrides.ts:54-57`); the module fallbacks use the
  same shape (`config/schema/eidnara.ts:128-133`).
- "Rejects the module keys as unknown": replaced. The summarizer schema
  declares `module_model`, `module_fallback_models`, and
  `context_limit_tokens` (`config/schema/eidnara.ts:121-141`), so
  `assertKnownConfigKeys` (`:782-790`) no longer throws for them.
- "Project sanitizer strips model, fallback, disallowed-tool, hidden-agent
  escalation, disable, and cost-cap fields": preserved and extended.
  `HISTORY_SUMMARIZER_USER_ONLY_FIELDS` (`config/project-security.ts:24-31`)
  adds the two module keys and `context_limit_tokens`;
  `AGENT_ESCALATION_FIELDS` (`:67`), `HIDDEN_AGENT_ACTIVATION_FIELDS` (`:23`),
  and `AGENT_COST_CAP_FIELDS` (`:33-39`) are applied in
  `stripUnsafeProjectConfigFields` (`:347-570`, loops `:518`, `:536`,
  `:548`, `:556`).
- Added by #898: the admission verdict and the withdrawal of fold authority
  for an unresolved configuration (see FA-N01). Pi shares the screen
  (`packages/pi-plugin/src/config/index.ts:94`).

Spec citations at `265df096` that moved: `config/index.ts:108-115` (the
substitution call) is now `:141-147`; `config/project-security.ts:23-36` is
`:23-39`; `:512-564` is `:515-567`. The existing checks moved:
`config/index.test.ts:695` ("leaves {env:} and {file:} tokens literal in
project config and warns") is now `:715`, and `:740` ("keeps
history_summarizer model selection user-owned when project config tries to
override it") is now `:760`. `config/config-paths.test.ts:66` is unchanged.

Gate results of the runs cited:

- #898 (merged head `4c964f93a`): `bun run check:repo`, smoke, schema
  regeneration and its exact-output test pass.
- #904 (branch head `57a820bb6`): `bun run check:repo` pass.

## Failure scenario

A surviving clause regresses: the project tier supplies a module model the
daemon ignores, or substitution resolves a `{file:}` chain value the daemon
excludes, and the plugin's predicate disagrees with the daemon's.

## Timing windows and dependencies

None; each load reads the files once at plugin startup.

## What a test must construct

A `.json`-only user tier; a user `{env:}`/`{file:}` outside the chain keys and
inside them; string and array fallbacks; module keys in the user tier and in
the project tier; each sanitizer field in the project tier.

## Investigation log

### Q: Is the project strip of the module keys witnessed?

- Sources examined: `config/project-security.test.ts`; `config/index.test.ts`;
  the fixture row "a model only in the project tier".
- Findings: No sanitizer unit test names `module_model`. The fixture row puts
  `model` and `module_model` in the project tier and expects `admitted`, an
  empty chain, and `false`; "every row matches through the production
  loader" (`config/index.test.ts:1114`) and the Pi row test
  (`packages/pi-plugin/src/config/index.test.ts:765`) pass it.
- Missing evidence: A warning assertion for the stripped module keys.
- Conclusion: resolved with answer; the fixture row witnesses the effect.

### Q: Is FA-E11 replaced or preserved?

- Sources examined: #834 D1a; #898's "Replaced property clauses".
- Findings: Replaced in part: module keys are accepted and chain keys are
  screened before substitution. Discovery, substitution elsewhere, fallback
  shape, and the sanitizer survive.
- Missing evidence: None.
- Conclusion: resolved with answer; invalidated with the surviving clauses
  listed.
