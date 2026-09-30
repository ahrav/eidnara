# fa-n02-first-commit-adopts-authority

## Discovery trigger

Proposed record FA-N02 of #834 (D3, C2; persistence handoff co-commit set 1:
adoption rides the first accepted core and metadata commit). Ticket #855
implements it; PR #903 lands it. Surfaces: daemon and store.

Exercised status: yes - the handler tests below construct each required
enabling state and ran green in the #903 `cargo test --workspace` gate and in
the #859 PR A full workspace run at `fd0b52aa5` (6,124 passed, 0 failed).

## Evidence trail

All references are verified at HEAD `0ff62b29a`.

- The stored fact is `ModuleMeta.eidnara_folds: Option<bool>` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`
  (`crates/memory-store/src/lib.rs:2460-2465`). An absent key reads as `None`,
  so a default never supplies `false`.
- `load_fold_authority` (`crates/memory-store/src/lib.rs:13200`) reads the key
  with `json_type` through `FOLD_AUTHORITY_SELECT` (`:6735-6739`) and does not
  decode the row. A non-boolean JSON type is `MemoryStoreError::Serde`
  (`:13226-13235`). `applied` is the stored value, or `Some(true)` for a row
  without the key that carries a fold artifact (`fold_artifacts_present`,
  `:6727-6733`): coverage, a folded history-segment sequence, or a
  non-Idle summarizer. Since `a694bdffa` (#903 on `main`) a stored block
  identity is not an artifact, because compaction-off passes also record
  identities, so a legacy row that holds only identities adopts its
  binding's intent; the adopting commit then clears those rows when the
  intent is native (`clear_identities`,
  `crates/daemon/src/transform.rs:3129`, since `ccab18208`).
- The pure planner `fold_authority::plan` (`crates/daemon/src/fold_authority.rs:95-139`)
  adopts the binding's intent for an unadopted, artifact-free row only when the
  configuration is admitted (`:105-111`), and adopts `applied` for a legacy
  row only when admitted (`:112`). An adopted row never gets `adopt`.
- The binding's intent is `DaemonConfig::eidnara_folds()`
  (`crates/daemon/src/config.rs:181-185`), which is false when admission is
  unresolved. It is captured per route in `RouteBindings::fold_authority_intent`
  (`crates/daemon/src/lib.rs:338-349`).
- `apply_once_with_estimator` reads the record and plans on every attempt
  (`crates/daemon/src/transform.rs:2022-2023`). `stamp_fold_authority`
  (`:2154-2168`) requires the pass's own load to be at the planned
  `row_version` and writes `adopt` into the metadata that the pass commits.
  Both paths call it: the additive path (`:2746`) and the folding path
  (`:3390`). A version mismatch is a `CasConflict`, which the attempt loop
  retries under a fresh plan (`:2110-2116`), bounded by `MAX_CAS_RETRIES = 8`
  (`:81`).
- A state-sync row carries no key: native state sync reads the field only to
  skip seeds (`crates/memory-store/src/lib.rs:11434-11439`).

Witnesses in `crates/daemon/src/fold_authority_handler_tests.rs` (#903):

- `the_first_committing_pass_adopts_over_a_state_sync_row` (`:135`): a real
  `state_sync` creates the row with `eidnara_folds == None`, then the first
  pass adopts `Some(false)` or `Some(true)` from the binding.
- `a_losing_first_adopter_converges_on_the_winner` (`:163`): a one-shot
  pre-commit hook commits `Some(true)` for a rival between the native pass's
  plan and commit. The loser adopts nothing and reports
  `fold authority pending native`.
- `a_fold_artifact_written_after_the_plan_replans_the_adoption` (`:742`): a
  non-Idle summarizer lands after the plan; the replan adopts Eidnara by the
  legacy rule. The PR records that the negative control fails without the
  row-version fence.
- `an_unresolved_configuration_adopts_nothing` (`:203`) and
  `legacy_rows_adopt_by_their_fold_artifacts` (`:329`), which also asserts
  that a row holding only identities adopts the binding and that native
  adoption leaves no identity rows (`:399-422`).
- Pure table: `the_transition_table_follows_the_session_authority_rules`
  (`crates/daemon/src/fold_authority.rs:200`), 128 combinations.
- Store: `a_non_boolean_fold_authority_is_a_serde_error_on_every_read`
  (`crates/memory-store/src/lib.rs:29410`).

## Failure scenario

Two OpenCode windows bind one fresh session with different chains. Both first
passes plan "unadopted". If the loser's commit carried its own intent, the
session's authority would depend on commit order, and a later bind could read
either value.

## Timing windows and dependencies

The window is between `load_fold_authority` and the pass commit. The
row-version fence in `stamp_fold_authority` closes it: any intervening write
changes `row_version`. A pass that commits nothing adopts nothing, because
adoption is a metadata change and rides the commit (comment at
`crates/daemon/src/transform.rs:2151-2153`).

## What a test must construct

A row created by state sync; two adopters with different intents where one
commits between the other's plan and commit; an unresolved configuration; a
row without the key, with and without artifacts; a non-boolean key.

## Investigation log

### Q: Is the competing adopter a second pass through the handler?

- Sources examined: `fold_authority_handler_tests.rs:163-201`;
  `transform.rs:3111-3114` (the hook site on the additive path).
- Findings: No. The rival is a direct `store.commit` of `Some(true)` from the
  pre-commit hook. It moves the row version exactly as a second pass would,
  so the fence is exercised, but no second handler pass runs concurrently.
- Missing evidence: A two-pass interleaving through two routes.
- Conclusion: resolved with answer; the direct commit is a sufficient
  enabling state for the fence, and the gap is recorded as a refinement.

### Q: Where does a row without the key come from at HEAD?

- Sources examined: PR #905 description (D5: older stores refused at open);
  the writers that commit without adopting.
- Findings: A store written before #905's baseline is refused, so pre-M1 rows
  cannot reach HEAD. A keyless row comes from state sync or from a pass under
  an unresolved configuration. The tests build legacy rows by clearing the
  field directly.
- Missing evidence: None for the property.
- Conclusion: resolved with answer.

### Q: Do #903's later commits on `main` change the adoption rule?

- Sources examined: `git show a694bdffa ccab18208`;
  `crates/memory-store/src/lib.rs:6727-6739`;
  `crates/daemon/src/transform.rs:3114-3133`;
  `crates/daemon/src/fold_authority_handler_tests.rs:329`.
- Findings: The guarantee holds. `a694bdffa` drops the `block_identities`
  probe from the fold-authority read, so stored identities alone no longer
  make a keyless row adopt Eidnara; such a row adopts its binding's intent.
  It also pins a pass to the binding it resolved: a pass whose route no
  longer holds that binding after its waits runs detached and neither plans
  an authority change nor settles another binding's first pass.
  `ccab18208` makes a native adoption release the row's identities in the
  adopting commit.
- Missing evidence: None.
- Conclusion: resolved with answer: evidence updated; the record stays
  `active`.
