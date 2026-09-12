# consolidated-cache-state-reads-match-per-consumer-loads

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The single-load evidence below describes the live pass.

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

[load]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6196-L6223
[full-select]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L4581-L4582
[meta-select]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L4579-L4580
[meta-epoch]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L1387-L1388
[meta-historian]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L1503-L1504
[phase]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L537-L546
[hds-state]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L579-L582
[unique]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3291-L3372
[recomp]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10057-L10150
[epoch-read]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4271-L4289
[epoch-read-delta]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4151-L4180
[epoch-comment]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4167
[epoch-load]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4168-L4173
[active]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4567-L4580
[prepare]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4994-L5067
[prepare-load]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5013
[no-fire-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5449
[no-fire]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5450-L5463
[floor-a]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8215-L8223
[hook]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8224-L8232
[rerun]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8268-L8271
[floor-b]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8358-L8377
[t-cas]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L22606
[t-emergency]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L34923
[t-no-fire]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L35691
[cas-retry]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1940-L1979
[descent-load]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2910
[descend]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2921-L2932
[snapshot]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2998
[truncate]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L4089-L4095
[truncate-reload]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L4107

## Single-load evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the
storage units that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/432)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

The handler [loads `meta` once][pass-load] through [`load_meta`][meta-load]
before the tail-delta expansion and hands the result to the pre-transform
consumers as a [`PassState`][pass-state]: `Loaded` carries the metadata,
`Unavailable` means the load failed, and `Reload` means the consumer runs after
a commit. The load reads the [`meta` projection][meta-select] alone: no
pre-transform consumer reads the core, and the full row's `core_state`
deserialization is the largest cost of a full load. The
[tail-delta expansion][delta] and the [projection-cache lookup][lookup] take
`Loaded`'s epoch and return `None` otherwise, so a failed load still answers
full-sync and misses the cache as it did on its own failed load. The
[last-response anchor][last-response] takes `Loaded`'s timestamp, `0` on
`Unavailable`, and a `load_meta` read on `Reload`; the
[historian-active check][active] takes `Loaded`'s phase when it is idle, idle on
`Unavailable`, and the phase alone from the store on `Reload` and on a `Loaded`
non-idle phase with no live run, since a run that completes after the pass load
leaves the live map and commits idle. The
[transform closure][run-transform] receives the pass state on its first run
and `Reload` on every rerun after an inline firing or a live completion, and
the load is dropped after the first run so a rerun can name nothing else.
The transform's own [snapshot][snapshot-live] stays its linearization point and
a CAS conflict still reloads; `prepare_historian_fire` keeps its post-commit
full load because its no-fire write needs the committed `row_version`. The
compartment state-sync gate keeps its full load as well: it writes on the
strength of that read and had no hot-path motive to narrow.

The pass load is timed on its own as the [`pass_state_load`][pass-timing]
pass-trace bucket. At the baseline the read sat inside the `delta_expand` and
`projection_cache_lookup` windows; moving it ahead of both would otherwise
shrink those buckets by the read's cost while `handler_total` kept it.

The read point of the historian-active check and the last-response anchor
moved earlier, from inside the transform closure to before the side-channel
drain and the pass-trace write. Neither of those writes `cache_state`, so no
in-process write is hidden; a concurrent publish landing inside that section is
read as the earlier phase only while a run is live. A live run is reported
active through the in-process live map before the durable phase is consulted;
a loaded active phase with no live run is re-read, so a run that completed
between the pass load and the check does not veto the pass.

Post-commit scalar consumers read one `meta` field by
[SQL JSON extraction][scalar-select]: `json_valid(meta, 1)` refuses text that
is not strict JSON, since SQLite's parser accepts JSON5 that serde refuses;
`json_type(meta)` refuses strict JSON that is not an object (`null`, a number,
an array), which deserializes to no `ModuleMeta` and would otherwise read every
path as absent; `json_type(meta, path)` tells an absent path, which takes the
serde default, from a JSON
`null`; and `->>` yields the unquoted value, whose type is checked against the
type text because `->>` renders a boolean as the integer `1` or `0`. The three
functions share one parse of `meta`: the bundled SQLite keys its JSON parse
cache on a negative auxdata slot, which every function in a statement sees. The
Emergency95 [floor reads][floor-live] share one
[`load_publication_floor_ordinal`][floor-accessor] closure; the wrapup epoch
check uses [`load_revert_epoch`][epoch-accessor]; `historian_active` uses
[`load_historian_phase`][phase-accessor] on `Reload` and on a loaded active
phase with no live run.

The [differential test][scalar-test] builds rows from a serialized default
`meta` and shows each accessor equal to the full deserialization where that
succeeds, and failing on its own field where the full load fails: a `null`,
negative, textual, or boolean `revert_epoch`; a boolean floor; an unknown
`historian.state`; JSON5; malformed text; and strict JSON that is not an
object. It also shows `load_meta` equal
to the full load's `meta` on every row both accept, and failing on every row
both refuse. The recorded divergences are per-column and per-field versus
per-row reading: a corrupt `core_state` fails only the full load, so
`load_meta` and every scalar accessor answer on such a row; a sibling field's
corruption fails only the full load and `load_meta`; a `historian` that is not
an object reads as an absent path, so the phase is idle where the full load
refuses the row; and an integer above `i64::MAX`, which SQLite returns as a
float, fails only the scalar read. A pre-transform consumer therefore proceeds
on a row whose `core_state` is corrupt where its own full load once refused
it; the transform's snapshot then fails that row and the pass is rejected
before any commit, so the consumers' work on that pass is discarded. Every
post-commit scalar consumer runs after a full load of the same row succeeded in
the same request or holds a value the transform already committed.

The [load-count test][load-count] arms the statement probe, runs a warm pass,
then a steady pass with the interleave hook sampling both projections' counts
between the transform commit and the post-commit load: one `meta` load and no
full load before the hook, one full load and no `meta` load after, and no
eviction of either counted handle. The counters key on the prepared statement
text through [`CacheStateSelect`][select-probe]; the
[counter test][counters-test] forces an eviction with a cache of one and shows
the counter records it. At the baseline the same pass ran the full select three
or four times. The [durable-phase test][phase-test] exercises `historian_active`
on each `PassState`; the [timing test][timing-test] shows the bucket present and
non-zero on a steady pass.

### Focused execution, 2026-09-12

`cargo test -p memory-store --locked` passed with the differential test and the
counter test; `cargo test -p daemon --locked` passed with the load-count test,
the timing test, the durable-phase test, and the extended interleave test, the
two `dreamer_run_task_bounds_*` tests failing under full-suite load on the base
branch as well and passing in isolation.

[pass-load]: ../../../../../crates/daemon/src/lib.rs#L8121
[meta-load]: ../../../../../crates/memory-store/src/lib.rs#L6660-L6672
[meta-select]: ../../../../../crates/memory-store/src/lib.rs#L4891-L4892
[pass-state]: ../../../../../crates/daemon/src/lib.rs#L3507-L3511
[delta]: ../../../../../crates/daemon/src/lib.rs#L4206
[lookup]: ../../../../../crates/daemon/src/lib.rs#L4317
[last-response]: ../../../../../crates/daemon/src/lib.rs#L4641-L4673
[active]: ../../../../../crates/daemon/src/lib.rs#L4609-L4632
[run-transform]: ../../../../../crates/daemon/src/lib.rs#L8222
[pass-timing]: ../../../../../crates/daemon/src/transform.rs#L1033-L1034
[snapshot-live]: ../../../../../crates/daemon/src/transform.rs#L3041
[scalar-select]: ../../../../../crates/memory-store/src/lib.rs#L4921-L4922
[floor-live]: ../../../../../crates/daemon/src/lib.rs#L8217
[floor-accessor]: ../../../../../crates/memory-store/src/lib.rs#L6760-L6775
[epoch-accessor]: ../../../../../crates/memory-store/src/lib.rs#L6722-L6734
[phase-accessor]: ../../../../../crates/memory-store/src/lib.rs#L6740-L6754
[select-probe]: ../../../../../crates/memory-store/src/lib.rs#L4896-L4912
[scalar-test]: ../../../../../crates/memory-store/src/lib.rs#L15725-L15944
[counters-test]: ../../../../../crates/memory-store/src/lib.rs#L15951-L15979
[load-count]: ../../../../../crates/daemon/src/lib.rs#L24695-L24747
[timing-test]: ../../../../../crates/daemon/src/lib.rs#L24753-L24765
[phase-test]: ../../../../../crates/daemon/src/lib.rs#L24771-L24784
