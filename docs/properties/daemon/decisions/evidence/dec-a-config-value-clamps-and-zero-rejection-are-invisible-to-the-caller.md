# dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller

## Discovery trigger

The property asks whether resolution reports a supplied value that it clamps or
discards. The reporting gap remains at
`74044960ee91641dec95c8552f15282844a18b13`; the claim that the entire warning
channel is absent does not. All Rust references below name
`crates/daemon/src/config.rs` at that revision.

## Evidence trail

The effective threshold is clamped to `[1, 90]` after merging
(`config.rs:750-752`). `apply_key` clamps the auto-search score to `[0.3, 0.95]`
(`config.rs:827-831`), minimum prompt characters to `[5, 500]`
(`config.rs:832-836`), and caveman minimum characters to `[100, 10000]`
(`config.rs:842-846`). None of those branches adds a range warning.

The standard memory budget, its legacy fallback, and the user-profile budget
apply only `.max(1.0)` (`config.rs:847-869`). The legacy spelling emits a
deprecation warning, not a range warning. Use the standard spelling to prevent
that unrelated warning from satisfying a range-reporting check.

`positive_usize_at` rejects zero (`config.rs:955-961`). This leaves the earlier
effective value unchanged for minimum prompt characters, caveman minimum
characters, and historian context limit (`config.rs:832-845`,
`config.rs:885-889`). With only a user tier and no earlier value, the defaults
survive. Minimum prompt characters defaults to `20` (`config.rs:35`,
`config.rs:56-61`); zero is not clamped to five.

The merge returns `(DaemonConfig, Vec<String>)` (`config.rs:710-755`). It warns
when a project supplies a user-only key or a changed weakening candidate. Those
are tier-policy diagnostics, not clamp diagnostics. File read/parse warnings
are added by `effective_with_warnings` (`config.rs:262-282`) and emitted on stderr
(`config.rs:250-257`, `config.rs:402-406`). Tests can inspect either warning
vector directly.

## Failure scenario

Resolve a user tier containing only
`memory.auto_search.min_prompt_chars: 0`. The effective value remains `20` and
the warning vector is empty. Resolve a user score threshold of `0.99` or caveman
minimum of `50`; the effective values become `0.95` and `100`, respectively,
without a key-specific range warning.

## Timing windows and dependencies

No timing fault is required. Supply one user-tier leaf, with the project tier
absent, to isolate parsing from tier rejection. For an allowed project leaf,
zero preserves the user value rather than necessarily restoring a default.

## What a test must construct

Compare each supplied out-of-domain leaf with its resolved value and require a
warning naming that leaf when the value is clamped or discarded. Do not accept
an ignored-project-key or legacy-key deprecation warning as proof that the
user-tier range was reported. A same-value input is a control, not a trigger.

`auto_search_and_caveman_config_follow_user_then_project_tiers`
(`config.rs:1407-1446`) checks ordinary overrides. `project_threshold_may_only_raise`
(`config.rs:1297-1302`) checks the upper threshold clamp but not its warning.
Both remain `unaudited`; neither is a clamp-reporting oracle.

## Investigation log

### Q: Does the classified merge close the reporting gap?

- Sources examined: `config.rs:710-755`, `config.rs:827-869`, and
  `config.rs:955-968`.
- Findings: policy warnings exist, but numeric clamps and zero rejection do not
  report the altered leaf.
- Missing evidence: a dedicated clamp-reporting check. Whether to require such
  diagnostics remains a policy question, not a runtime change in this catalog.
- Conclusion: resolved with answer for behavior; the reporting property remains
  active.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller.md)
preserves the source-catalog documentation comparisons. Its removed helper names,
six-site warning count, and claim that no test can observe warnings are not
current evidence.
