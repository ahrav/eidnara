# dec-a-model-chain-dedup-is-adjacent-only

## Discovery trigger

The source catalog identified an adjacent-only `Vec::dedup` call on an ordered
historian fallback chain. That defect premise is invalidated by the implementation
at `74044960ee91641dec95c8552f15282844a18b13`. The record retains the uniqueness
guarantee and its existing check; it is not an open defect campaign.

All source references below name `crates/daemon/src/config.rs` at that revision.

## Evidence trail

`merge_tiers_with_warnings` calls `dedup_preserving_order` after applying both
tiers (`config.rs:750-754`). The helper inserts each model into a `HashSet` and
retains only the first insertion (`config.rs:950-953`). It removes both adjacent
and non-adjacent repeats without sorting the chain.

`apply_key` trims model strings and ignores empty strings
(`config.rs:761-772`). A non-empty module model selects the module fallback list;
otherwise the plugin model and fallback list apply (`config.rs:774-806`). All
four model keys are user-only (`config.rs:659-672`), so a project cannot add a
second chain. Its supplied model keys produce ignored-key warnings instead
(`config.rs:723-733`).

`model_chain_drops_repeats_anywhere_and_keeps_first_occurrence_order`
(`config.rs:2185-2194`) resolves primary `a` and fallbacks `[b, a, c, b]`, then
asserts `[a, b, c]`. Status: `unaudited`. This is a check inventory entry, not an
adequacy verdict for every model-key spelling.

## Failure scenario

The discovery input, module primary `a` with module fallbacks `[b, a]`, reaches
the same helper and resolves to `[a, b]`. The former prediction `[a, b, a]` is
false. A regression that replaces the helper with adjacent-only deduplication
would restore the defect; the uniqueness guarantee remains relevant to that
regression.

## Timing windows and dependencies

There is no timing window. Reachability is `explicit-config-only`: the default
chain is empty (`config.rs:114`), and a repeated model requires user configuration.
Deduplication applies on every tier merge, including cached file resolutions.

## What a test must construct

Retain the existing non-adjacent-repeat and ordering check. The module-key
precedence checks (`config.rs:1935-1977`) and project rejection check
(`config.rs:1980-1998`) exercise separate concerns. Each has status `unaudited`.
Do not design a campaign around a nonexistent adjacent-only call or the former
cache-TTL test coordinates.

## Investigation log

### Q: Does the adjacent-only defect remain?

- Sources examined: `config.rs:750-754`, `config.rs:950-953`, and
  `config.rs:2185-2194`.
- Findings: full deduplication preserves first occurrence, and an existing test
  supplies non-adjacent repeats.
- Missing evidence: none for the implementation mechanism. Test adequacy remains
  unaudited.
- Conclusion: resolved with answer. The defect premise is invalidated by code,
  not merely by the existence of a test.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-model-chain-dedup-is-adjacent-only.md)
preserves the source-catalog investigation and its old line coordinates. Those
coordinates and conclusions are historical, not current evidence.
