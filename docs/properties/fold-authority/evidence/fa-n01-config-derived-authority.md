# fa-n01-config-derived-authority

## Discovery trigger

Proposed record FA-N01 of #834 (D1, D1a, D1b; C7). Ticket #852 implements it;
PR #898 lands it together with M0. Surfaces: daemon, plugin, pi, cli.

Exercised status: yes - one 49-row fixture runs through the daemon's
production file path, the OpenCode loader, and the Pi loader, and each suite
ran green in a recorded run (#859 PR A at `fd0b52aa5` for Rust; #904's
`bun run check:repo` for TypeScript).

## Evidence trail

Package references are verified at `d7b330113`, the package tree of #859
PR C's head; Rust references are verified at `0ff62b29a`.

- Daemon predicate: `DaemonConfig::eidnara_folds`
  (`crates/daemon/src/config.rs:181-185`) is `admission == Admitted &&
  compaction_enabled && !model_chain.is_empty()`. The default config has an
  empty chain and `compaction_enabled: true` (`:142-144`).
- Daemon resolution: `ConfigCache::effective_with_warnings` (`:310-380`)
  discovers `.jsonc` then `.json` (`discover_tier`, `:401-408`), collects
  authority rejections (`authority_rejections`, `:1115-1183`), and on a
  rejection keeps the last admitted tier documents or, before any admission,
  the tier minus its authority blocks (`:332-360`, `without_authority_blocks`
  `:1093-1099`). Chain keys go through `chain_models` (`:1064-1082`) and
  `literal_chain_model` (`:1051-1054`); module precedence is
  `module_model_selected` (`:1084-1089`) applied in `apply_key` (`:868-883`).
  Production reaches it through `effective_for_project`
  (`crates/daemon/src/lib.rs:4744`).
- TypeScript predicate: `isCompactionEnabled`
  (`packages/opencode-plugin/src/config/agent-disable.ts:21-29`) is
  `compaction.enabled !== false && normalizeSummarizerChain(...).length > 0`.
  The chain normalizer is `normalizeSummarizerChain`
  (`packages/opencode-plugin/src/config/fold-authority.ts:22-32`).
- TypeScript resolution: the user tier is screened before substitution
  (`screenUserTier`, `fold-authority.ts:97-121`, called at
  `config/index.ts:139`); excluded chain literals are blanked at their token
  span (`excludedChainValues`, `fold-authority.ts:79-95`). Admission is
  `admissionOf` (`config/index.ts:602-607`); an unresolved admission sets
  `compaction.enabled = false` (`withdrawUnresolvedFoldAuthority`,
  `fold-authority.ts:137-146`, called at `config/index.ts:561-564`). Pi uses
  the same screen and withdrawal (`packages/pi-plugin/src/config/index.ts:94`,
  `:480-485`).
- The fixture's `contract` field defines the supported-input limit (regular
  UTF-8 files of at most 1 MiB, final component not a symlink, valid JSONC
  before substitution) and says an unresolved row "leaves folding to the
  host's native compaction, so expected_eidnara_folds is false". CI's `test`
  filter names the fixture (`.github/workflows/ci.yml:94`).
- Negative controls: `the_pre_parsed_merge_seam_fails_the_fixture`
  (`config.rs:2573`), `jsonc_only_discovery_fails_the_fixture` (`:2588`), and
  "rejects a raw lenient read that skips the loader pipeline"
  (`packages/opencode-plugin/src/config/index.test.ts:1179`).

Gate results of the runs cited, as recorded:

- #898 (merged head `4c964f93a`, merge `dd5dcc170`): fmt, clippy, rustdoc,
  `cargo test --workspace`, comment markers, `bun run check:repo`, smoke, and
  schema regeneration pass. The description predates `f5b10fefe` ("Leave
  folding to native compaction when the configuration is unresolved"), which
  merged with the PR.
- #904 (branch head `57a820bb6`): `bun run check:repo`, smoke, markers, fmt,
  clippy pass; `cargo test --workspace` pass except one flaky untouched
  host-runtime test, green on rerun. After it, `packages/` takes the review
  changes merged through `main` (fold-authority warning delivery and
  reconciliation), and `d7b330113` changes only comments and generated TUI
  source (`plugin/rpc-handlers.ts`, `tui/index.tsx`, `tui-compiled/index.tsx`).
- #859 PR A run at `fd0b52aa5`: 6,124 passed, 0 failed, 66 ignored; fmt, clippy,
  rustdoc, markers green. Every `config::fold_authority_parity` test is in it.

## Failure scenario

The daemon resolves a non-empty chain while the plugin resolves an empty one
(or the reverse). The daemon folds while OpenCode's compaction is also on, or
neither folds while `setup` has turned OpenCode's compaction off.

## Timing windows and dependencies

A rejected reload in a long-running daemon: the daemon keeps retained tier
documents, while the plugin loads once at startup. Both then withdraw the
fold authority for the unresolved state.

## What a test must construct

Every fixture row on disk under an isolated environment, loaded through each
production loader, comparing chain, admission, and boolean; one rejected
reload after an admitted load; and a negative control that bypasses the file
path and must fail.

## Investigation log

### Q: Does returning `false` for an unresolved configuration satisfy D1a?

- Sources examined: D1a in #834; `config.rs:181-185`; `fold-authority.ts:137-146`;
  the fixture contract; `fold_authority::plan`
  (`crates/daemon/src/fold_authority.rs:105-113`); setup
  (`packages/cli/src/commands/setup-opencode.ts:618-628`); doctor
  (`packages/cli/src/commands/doctor-opencode.ts:539-542`).
- Findings: D1a says a rejected admission is "never `false`". Both loaders
  return `false` and carry `Unresolved` beside it. The consumers found read
  the admission first: the daemon plan adopts nothing and changes nothing
  while unadmitted, setup stops before any host edit, and doctor fails the
  check. `an_unresolved_configuration_withdraws_fold_authority_from_the_retained_documents`
  (`config.rs:2711`) pins the daemon side.
- Missing evidence: The owner's reading of the falsifier.
- Conclusion: needs human input.

### Q: Does the CLI run the fixture?

- Sources examined: `packages/cli/src/lib/eidnara-modes.ts:17-65`.
- Findings: No. The CLI reads the user tier alone through
  `loadUserTierConfigDetailed` by design (setup edits global host settings),
  so project-tier rows do not apply to it. `eidnara-modes.test.ts:24`, `:53`,
  and `:68` cover its path. `compactionEnabledFor` and `foldAuthorityOf` also
  answer native for a top-level `enabled: false`, which the daemon predicate
  does not read; no fixture row has that key.
- Missing evidence: None for the contract as written.
- Conclusion: resolved with answer; the parity claim covers the daemon,
  OpenCode, and Pi loaders.
