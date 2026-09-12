# consolidated-cache-state-reads-match-per-consumer-loads

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

One pass loads `cache_state` several times: before the transform for the
projection-cache epoch and for `historian_active`, inside the transform for
the snapshot, and after it inside `prepare_historian_fire`, with two more
loads on the Emergency95 arm. The audit proposes one load per pass or narrow
scalar reads of `meta`. A consolidation can hand a pre-commit snapshot to a
consumer placed after the pass's own commit, and a narrow read can decode a
defaulted or malformed field differently from the full deserialization.

## Evidence trail

- [`MemoryStore::load`][load] runs [`CACHE_STATE_FULL_SELECT`][full-select]
  in one read and deserializes `core_state` and `meta` with serde; either
  failure returns `Err`, so a corrupt `core_state` fails a meta-only consumer.
- Pre-commit loads: [`lookup_full_projection_cache`][epoch-read] reads
  `meta.revert_epoch` with `.ok()?`, so any load error yields `None`;
  [`expand_transform_tail_delta`][epoch-read-delta] does the same at
  [`:4168-4173`][epoch-load] with the [comment][epoch-comment] that the epoch
  must be the persisted one; [`historian_active`][active] maps a load error to
  `false` and otherwise tests `meta.historian.state != Idle`.
- Inside the transform, the lineage-switched path [loads][descent-load],
  [descends][descend] with `expected_target_row_version`, then takes the
  [snapshot][snapshot]; the revert path [truncates][truncate], sets
  `commit_expected` to the returned `row_version`, and
  [reloads compartments][truncate-reload].
- Post-commit: [`prepare_historian_fire`][prepare] loads at
  [`:5013`][prepare-load] and passes `loaded` to [`record_no_fire`][no-fire],
  which commits `last_no_fire` under `loaded.row_version` with `let _ =`, so a
  stale `row_version` fails the CAS silently. Its doc comment reads
  [`/// Delete`][no-fire-doc], which describes nothing the function does.
- Emergency95: [`emergency_pre_floor`][floor-a] loads
  `publication_floor_ordinal` after the transform, again after an awaited
  inline firing ([`:8268-8271`][rerun]), and the final check at
  [`:8358-8377`][floor-b] loads once more and reruns the transform when the
  floor differs. The two reads are the rerun trigger.
- A CAS conflict makes [`apply_once_with_estimator_and_projection`][cas-retry]
  rerun `apply_once`, which reloads.
- Serde defaults: [`revert_epoch`][meta-epoch] and
  [`historian`][meta-historian] carry `#[serde(default)]`, as does
  [`HistorianDurableState::state`][hds-state];
  [`HistorianPhase`][phase] is `snake_case` with five variants and no
  catch-all, so an unknown string fails serde. A `null` under a `u64` fails
  serde. A JSON path extract returns NULL for an absent key and a string for
  an unknown variant.
- [`CACHE_STATE_META_SELECT`][meta-select] exists but its users deserialize
  the whole `ModuleMeta`; no scalar projection of `meta` exists at HEAD.
- The [interleave hook][hook] at `:8224-8232` is `#[cfg(test)]` and runs
  between the transform and the floor check.

## Failure scenario

A consolidation reuses the pre-transform snapshot inside
`prepare_historian_fire`: `record_no_fire` writes under the old `row_version`,
the CAS fails, and the reason is never persisted. It reuses the first
`run_transform` snapshot after an inline firing, so the floor comparison at
`:8358-8377` compares equal values and the rerun never happens. A narrow read
returns NULL for an absent `revert_epoch` where serde yields `0`, selects a
projection-cache entry the full load would not, or reads `idle` from a row
whose `core_state` is corrupt where the full load returns `false`.

## Timing windows and dependencies

The freshness clause depends on the window between `commit_transform` and each
post-commit read, which C5 asks a campaign to reach. The decode clause has no
timing; it depends on the serde attributes above and on
[`parse_json_with_unique_names`][unique] keeping stored `meta` free of
duplicate names.

## What a test must construct

A pass that commits and then reaches `prepare_historian_fire` with a new
no-fire reason; an Emergency95 pass with a publication landing between the
transform and the floor check through the [hook][hook]; a CAS conflict
injected between snapshot and commit; rows whose `meta` lacks `revert_epoch`
or `historian`, carries an unknown `historian.state`, or holds `null` under
`revert_epoch`; rows whose `core_state` is not valid JSON. Pair any narrow read
with `MemoryStore::load` over those rows and assert equal values or the same
conservative branch. The
[state checks](../existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
cover the no-fire CAS ([`t-no-fire`][t-no-fire]), the emergency rerun
([`t-emergency`][t-emergency]), and the CAS retry ([`t-cas`][t-cas]); none
covers narrow-read equivalence or `historian_active` on durable state.

## Investigation log

### Q: Which pre-commit loads may share one snapshot?

- Sources examined: [`epoch-read`][epoch-read], [`epoch-load`][epoch-load]
  with its [comment][epoch-comment], [`historian_active`][active], the
  [snapshot][snapshot].
- Findings: The three loads are independent reads today; merging them closes
  a window in which the epoch is read before a concurrent recut. Nothing states
  whether that window is meant to stay open.
- Missing evidence: A specification statement.
- Conclusion: needs human input.

### Q: Must a narrow read fail the same way on a corrupt `core_state`?

- Sources examined: [`load`][load], [`historian_active`][active],
  [`epoch-read`][epoch-read].
- Findings: Every consumer takes a conservative branch on any load error, and
  the full load fails on either column. A meta-only read would proceed.
- Missing evidence: A decision on whether proceeding is acceptable.
- Conclusion: needs human input.

### Q: Does SQLite JSON path extraction return the first or the last duplicate?

- Sources examined: [`parse_json_with_unique_names`][unique],
  [`reset_session_for_recomp`][recomp].
- Findings: Every `meta` writer through `commit_transform` refuses duplicates;
  `reset_session_for_recomp` writes a self-generated `ModuleMeta` without
  scanning. No caller-influenced `meta` writer bypasses the unique-name parse.
- Missing evidence: SQLite behavior was not run; relevant only under a bypass.
- Conclusion: unresolved, needs a SQLite run only if a bypassing writer exists.

[load]: ../../../../../crates/memory-store/src/lib.rs#L6302-L6329
[full-select]: ../../../../../crates/memory-store/src/lib.rs#L4666-L4667
[meta-select]: ../../../../../crates/memory-store/src/lib.rs#L4664-L4665
[meta-epoch]: ../../../../../crates/memory-store/src/lib.rs#L1472-L1473
[meta-historian]: ../../../../../crates/memory-store/src/lib.rs#L1588-L1589
[phase]: ../../../../../crates/memory-store/src/lib.rs#L622-L631
[hds-state]: ../../../../../crates/memory-store/src/lib.rs#L664-L667
[unique]: ../../../../../crates/memory-store/src/lib.rs#L3376-L3457
[recomp]: ../../../../../crates/memory-store/src/lib.rs#L10163-L10256
[epoch-read]: ../../../../../crates/daemon/src/lib.rs#L4284-L4302
[epoch-read-delta]: ../../../../../crates/daemon/src/lib.rs#L4164-L4193
[epoch-comment]: ../../../../../crates/daemon/src/lib.rs#L4180
[epoch-load]: ../../../../../crates/daemon/src/lib.rs#L4181-L4186
[active]: ../../../../../crates/daemon/src/lib.rs#L4580-L4593
[prepare]: ../../../../../crates/daemon/src/lib.rs#L5007-L5080
[prepare-load]: ../../../../../crates/daemon/src/lib.rs#L5026
[no-fire-doc]: ../../../../../crates/daemon/src/lib.rs#L5462
[no-fire]: ../../../../../crates/daemon/src/lib.rs#L5463-L5476
[floor-a]: ../../../../../crates/daemon/src/lib.rs#L8228-L8236
[hook]: ../../../../../crates/daemon/src/lib.rs#L8237-L8245
[rerun]: ../../../../../crates/daemon/src/lib.rs#L8281-L8284
[floor-b]: ../../../../../crates/daemon/src/lib.rs#L8371-L8390
[t-cas]: ../../../../../crates/daemon/src/lib.rs#L22957
[t-emergency]: ../../../../../crates/daemon/src/lib.rs#L35324
[t-no-fire]: ../../../../../crates/daemon/src/lib.rs#L36092
[cas-retry]: ../../../../../crates/daemon/src/transform.rs#L1948-L1987
[descent-load]: ../../../../../crates/daemon/src/transform.rs#L2918
[descend]: ../../../../../crates/daemon/src/transform.rs#L2929-L2940
[snapshot]: ../../../../../crates/daemon/src/transform.rs#L3006
[truncate]: ../../../../../crates/daemon/src/transform.rs#L4097-L4103
[truncate-reload]: ../../../../../crates/daemon/src/transform.rs#L4115
