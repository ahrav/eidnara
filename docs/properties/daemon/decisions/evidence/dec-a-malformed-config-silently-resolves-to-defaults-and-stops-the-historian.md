# dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian

## Discovery trigger

The source catalog described read and parse errors collapsing into silent absence.
That defect premise is invalidated at
`74044960ee91641dec95c8552f15282844a18b13`. Ignoring an unusable tier remains the
fallback policy; silence does not. The diagnostic guarantee and existing check
remain recorded without prescribing a last-known-good policy.

All source references below name `crates/daemon/src/config.rs` at that revision.

## Evidence trail

`read_tier_cached` stores both `value` and `warning` (`config.rs:362-393`). A
parse failure produces a path-bearing `not valid JSONC` warning. A read failure
other than `NotFound` produces a path-bearing `could not be read` warning. A
missing file produces neither a value nor a warning (`config.rs:370-391`).

`effective_with_warnings` merges usable values, then appends both tier warnings
(`config.rs:262-282`). `effective_for_user_path` sends the vector to
`emit_warnings` (`config.rs:250-257`), which prints each warning on stderr
(`config.rs:402-406`). The private warning-returning method is directly callable
from the config test module; no signature change is needed for an oracle.

The effective config is replaced on every resolution (`config.rs:281-282`), not
retained as last-known-good after a failed read. An unusable user tier therefore
contributes no model chain, whose default is empty (`config.rs:114`). This
fallback is not evidence that the warning is absent.

## Failure scenario

Write an unterminated JSON object to the user file and use a directory as the
project config file. Resolution ignores both values but reports both paths and
failure kinds. Removing both paths leaves ordinary missing tiers and no warning.
The old assertion that those cases are indistinguishable is false.

## Timing windows and dependencies

The mtime fast path requires `cache.warning.is_none()` (`config.rs:363-366`).
Failed reads and parses are retried even when mtime is unchanged, and warnings
remain visible on repeated failures. Repairing an invalid file without changing
mtime can therefore recover. A successfully cached file whose bytes change while
mtime remains identical is a different case: it can still use the cached value.

## What a test must construct

`unreadable_and_malformed_tiers_warn_while_missing_tiers_stay_silent`
(`config.rs:2133-2182`) already constructs the malformed user file and directory
at the project path. It asserts fallback, path-specific warnings, repeated
warnings, same-mtime repair, and silent missing files. Status: `unaudited`.

`mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change`
(`config.rs:2093-2130`) separately asserts the successful-cache behavior.
Status: `unaudited`. Neither inventory entry certifies how a particular launcher
displays daemon stderr to a user.

## Investigation log

### Q: Is a diagnostic channel missing?

- Sources examined: `config.rs:262-282`, `config.rs:362-406`, and
  `config.rs:2133-2182`.
- Findings: tier errors are stored, collected, and emitted. A test can observe
  the returned warnings before emission.
- Missing evidence: launcher-specific stderr presentation is outside this
  config-reader claim.
- Conclusion: resolved with answer. The silent-file-failure premise is
  invalidated; fallback to usable tiers and defaults remains intentional code
  behavior, not a missing signal.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian.md)
preserves the old investigation. Its claims about `.ok()`, absent warnings, and
failed reads remaining cached until mtime changes do not describe this revision.
