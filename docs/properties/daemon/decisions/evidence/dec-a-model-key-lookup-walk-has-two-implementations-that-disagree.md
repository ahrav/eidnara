# dec-a-model-key-lookup-walk-has-two-implementations-that-disagree

## Discovery trigger

The source catalog stated one progressive model-key lookup contract for cache
TTLs and execute thresholds. The two Rust walks still differ at
`74044960ee91641dec95c8552f15282844a18b13`. The claim that no test constructs a
per-model threshold is false: the scheduler golden deserializes that variant.
Rust source paths below are relative to `crates/daemon/src/`.

## Evidence trail

`resolve_cache_ttl_with_provenance` checks the whole key, qualified and bare
dash-stripped variants, then `provider/*`, then the default
(`config.rs:151-207`). Bare keys also walk the dash ladder. The wildcard step is
at `config.rs:201-205`; the earlier source-catalog coordinates no longer name it.

`model_key_lookup_order` generates qualified and bare dash-stripped candidates
but no provider wildcard (`scheduler.rs:810-830`). Its two consumers fall back
to `default` after those candidates (`scheduler.rs:779-808`). A qualified model
key and a map containing only `provider/*` therefore distinguish the walks.

The daemon's config reader accepts only a numeric execute threshold
(`config.rs:807-811`, `config.rs:963-968`). Its transform adapter constructs
`ExecuteThresholdConfig::Percentage` and sets token thresholds to `None`
(`transform.rs:5447-5454`). A project or user JSON object at that config key does
not select the scheduler's map path.

The map path does have test exercise. `ThresholdCase` deserializes an
`ExecuteThresholdConfig` (`scheduler.rs:923-932`), and the golden test loads
those cases (`scheduler.rs:1011-1013`) before calling the threshold resolver
(`scheduler.rs:1087-1096`). The exact and derived-bare cases contain object-valued
percentage configs (`crates/daemon/testdata/scheduler-golden.json:87-106`). A
search for constructor syntax alone misses this exercise.

## Failure scenario

For `anthropic/model`, a TTL map with `anthropic/*` resolves that entry. A
scheduler threshold map with the same wildcard instead takes its default. This
is a direct-call differential, not a claim that the daemon's scalar config route
selects different live thresholds.

## Timing windows and dependencies

No timing fault is required. The catalog's `test-only` label describes the
in-tree differential: both walks can be called from crate tests, while the daemon
config route builds a scalar threshold. The scheduler module is public
(`lib.rs:31`); this label does not prove that an external library caller cannot
construct `ByModel`.

## What a test must construct

Use equivalent wildcard-only maps with distinct wildcard and default values.
Compare which entry each walk selects for the same qualified model key. Keep
exact, bare, and dash-stripped cases as controls.

Existing checks remain `unaudited`: config tests exercise object TTL routing
(`config.rs:1089-1115`) and retained shared vectors (`config.rs:1133-1160`);
the scheduler golden exercises its own threshold maps. None of these checks
compares the two Rust walks on the same wildcard-only map.

## Investigation log

### Q: Is the scheduler map path unconstructed?

- Sources examined: `scheduler.rs:923-932`, `scheduler.rs:1011-1013`,
  `scheduler.rs:1087-1096`, and the golden map cases above.
- Findings: deserialization constructs map variants in the golden test. The
  daemon config adapter still constructs only a scalar threshold.
- Missing evidence: a direct differential check of the wildcard step.
- Conclusion: resolved with answer. Test exercise exists; the differential gap
  and scalar production-route distinction remain.

### Q: Should both walks share wildcard semantics?

- Sources examined: both implementations and the source-catalog contract.
- Findings: the wildcard difference is observable. The original
  `CONFIGURATION.md` is absent from this repository.
- Missing evidence: confirmation of the intended cross-surface wildcard policy.
- Conclusion: needs human input. This refresh changes neither implementation.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-model-key-lookup-walk-has-two-implementations-that-disagree.md)
preserves the original contract quotation and investigation. Its coordinates,
blanket no-construction claim, and older bare-key algorithm are historical only.
