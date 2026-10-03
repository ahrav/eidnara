# fa-e10-rust-model-resolution-is-user-jsonc-only

## Discovery trigger

Existing-behavior record FA-E10 of #834 (first comment), describing the
daemon loader at `265df096`. #834 D1a replaces part of it; PR #898 (#852)
lands the replacement and names FA-E10 "replaced" in its description; FA-N01
is the replacing obligation. Surface: daemon.

Exercised status: partial - the surviving clauses (user-tier-only chain,
module precedence, no substitution, the 1 MiB bound) have named tests that
ran in the #859 PR A run at `fd0b52aa5`; the final-symlink rejection has none.

## Evidence trail

All references are verified at HEAD `0ff62b29a` unless another tree is named.

Clauses and their disposition at HEAD:

- "Discovers `.jsonc` tiers only": replaced. `discover_tier`
  (`crates/daemon/src/config.rs:401-408`) returns `.jsonc`, then `.json`,
  then the `.jsonc` path for a missing tier. Negative control:
  `jsonc_only_discovery_fails_the_fixture` (`:2588`).
- "Parses without variable substitution": preserved. `read_tier_cached`
  parses `strip_jsonc(&raw)` with `serde_json` and no substitution
  (`:461-492`, parse at `:470`). A chain value that contains `{env:` or
  `{file:` is now excluded with a warning (`literal_chain_model`,
  `:1051-1054`), and a user-tier key holding a reference is an authority
  rejection (`:1121-1132`).
- "Bounds a tier to 1 MiB and rejects a final-component symlink": preserved.
  `MAX_CONFIG_TIER_BYTES` is `1 << 20` (`:412`); `read_bounded_bytes`
  (`:427-459`) opens with `NOFOLLOW | NONBLOCK`, refuses a non-regular file,
  and counts bytes read. The fixture contract now records both as the
  supported-input limit for parity.
- "Derives the chain only from user-tier keys": preserved. The four chain
  keys are `TierClass::UserOnly` (`tier_class`, `:762-778`); the project pass
  warns and skips them (`merge_tiers_with_warnings`, `:829-837`).
- "Module-model precedence": preserved (`apply_key`, `:868-883`;
  `module_model_selected`, `:1084-1089`).
- "Array-only fallbacks": replaced. `chain_models` (`:1064-1082`) accepts a
  string or an array; a non-string element or other type is an authority
  rejection (`:1167-1182`).
- Added by #898: the admission verdict (`ConfigAdmission`, `:130-137`),
  last-admitted retention (`:332-360`), and `eidnara_folds` (`:181-185`).
- "Defaults allow an empty chain with compaction true": preserved
  (`:142-144`).

Spec citations at `265df096` that moved: defaults `config.rs:123-125` are now
`:142-144`. The four existing checks moved:
`tier_policy_ignores_project_models_and_rejects_project_lowering` from `:1317`
to `:1509`; `compaction_enabled_defaults_true_and_is_user_tier_only` from
`:1424` to `:1616`;
`module_model_keys_replace_the_plugin_chain_only_when_the_module_model_is_set`
from `:2045` to `:2237`;
`mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change` from
`:2158` to `:2350`. The source ranges moved: `:274-404` (resolution and
cached read) is now `:310-492`; `:325-370` (read bound) is `:412-459`;
`:675-691` (`tier_class`) is `:762-778`; `:772-824` (chain dedup and the
chain arms of `apply_key`) is `:859-883`, with the per-value rules moved to
`chain_models` (`:1064-1082`).

Gate results of the runs cited:

- #898 (merged head `4c964f93a`): `cargo test --workspace` and the other
  Rust gates pass.
- #859 PR A run at `fd0b52aa5`: 6,124 passed, 0 failed, 66 ignored. It lists every
  test named above plus `an_oversized_tier_file_is_ignored_with_a_warning`
  (`:1420`).

## Failure scenario

A surviving clause regresses: a project tier contributes a model, or the
daemon reads a symlinked or oversized project tier the plugin also reads, and
the daemon's chain diverges from the plugin's.

## Timing windows and dependencies

A tier edited between reads: the mtime cache reuses an unchanged read and
retries a failed one (`read_tier_cached`, `:461-466`).

## What a test must construct

A project tier supplying chain keys; a module model with and without a
blank value; a tier one byte over 1 MiB; a tier whose final path component
is a symlink to a valid file.

## Investigation log

### Q: Does any test witness the final-symlink rejection?

- Sources examined: `config.rs` tests; `git grep` for `symlink` in
  `crates/daemon/src`.
- Findings: No. `an_oversized_tier_file_is_ignored_with_a_warning`
  (`:1420`) covers the byte bound and a FIFO, not a symlink. The plugin
  follows symlinks, so a symlinked tier is outside the parity contract.
- Missing evidence: A symlinked-tier test through `effective_for_env`.
- Conclusion: unresolved, needs a symlink witness.

### Q: Is FA-E10 replaced or preserved?

- Sources examined: #834 D1a; #898's "Replaced property clauses"; the code
  above.
- Findings: Replaced in part. Discovery and fallback shape changed; the
  read policy, user-tier restriction, module precedence, and absence of
  substitution survive. FA-N01 owns the combined obligation.
- Missing evidence: None.
- Conclusion: resolved with answer; invalidated with the surviving clauses
  listed.
