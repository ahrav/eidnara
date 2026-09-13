# stage-timing-fields-keep-their-boundaries

## Rebase status, 2026-09-13

Relocation anchors refer to the formatted working tree atop `e451a2b4`.
The merged source preserves upstream's metadata-only read and `pass_state_load`
field; #438 does not introduce that field. The updated table describes source
boundaries, not a benchmark result. W2 remains invalidated.

## Historical baseline, 2026-09-10

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The following discovery and investigation retain their baseline scope. W2 is
invalidated in the catalog; the implementation note does not reactivate it.

## Discovery trigger

The per-stage timings are the only production numbers a latency change can
point to. The wildcard pass asked what protects the meaning of a field when
the work it brackets moves. It found that every field defaults to zero on
deserialization, that the handler-level pairs are taken on the handler task,
and that the token-cache delta is a thread-local subtraction, so a relocation
can shrink a number without making anything faster.

## Evidence trail

- [`TransformTimings`][tt] has 89 fields, each with `#[serde(default)]`, so
  a field removed from the producer deserializes as `0` or `0.0` on the
  consumer.
- [`format_pass_timing_line`][fmt] prints 90 `key=value` pairs (`session`
  plus one per field). The key for `post_attach` is `post_attach_ms`
  ([`:1254`][fmt-post-attach], value at [`:1331`][fmt-post-attach-value]);
  every other key equals its field name. The plugin's `rust module stages:`
  line ([`:1013-1042`][ts-stages]) reads 22 keys by name from the response
  object; all 22 exist in the struct. Its [`stage`][ts-stage-fn] helper
  prints `n/a` only when the key is absent or non-finite, so a present zero
  prints as `0.0`.
- The handler assigns 24 fields at [`:8463-8488`][h-timings]. Thirteen are
  millisecond durations from `Instant` pairs taken on the handler task:
  `handler_total` from `handler_started_at`, the request-to-handler gap,
  delta expand, side-channel drain, receive trace, cache lookup and store,
  native attach, completion trace, observation, retained size, snapshot
  store, and `post_attach`. Six trigger fields come from
  `HistorianTriggerTimings` filled inside [`prepare_historian_fire`][prepare];
  five are native-cache counts.
- [`record_token_cache_delta`][rtcd] subtracts two reads of
  [`local_stats`][tc-local], a `thread_local!` counter whose doc says the
  counters exclude other threads and only differences are meaningful. The
  start read sits at [`apply_additive_only:2373`][snap-add] and
  [`apply_once:2852`][snap-once]; both functions are synchronous today.
- [`respond_transform`][respond] hands `pass_timings` to
  [`emit_pass_timing`][emit], which sets the three response fields and prints
  the line for every response that carries `timings`.
- The plugin reads `handler_total` and falls back to `total` for its elapsed
  figure at [`:999-1012`][ts-read] and uses the values only for logging. The
  wire contract has no `timings` statement; the field is a daemon-to-plugin
  convention.

## Failure scenario

`run_transform` moves to a blocking thread. The handler's `Instant` pairs
still bracket handler-task work, so `projection_cache_store` and
`native_attach` stay honest, but `tokenize_calls` and its siblings now
subtract a worker thread's counter from the handler thread's counter and
report a fragment or zero. A merged stage is removed from the producer but
not the struct; the plugin prints `0.0` for it and a reader takes the drop
as an improvement.

## Timing windows and dependencies

None in time. The hazard is structural: a stage split across an `.await`
or moved to another thread changes what its field brackets while the field
keeps its name and its `#[serde(default)]` fallback.

## What a test must construct

An ordinary pass that populates `timings`, then a source-level assertion that
the plugin's 22 keys and the line's 90 keys resolve to struct fields (the
`post_attach_ms` rename is the one exception to record), and a per-field
statement of the start and stop instants each field brackets. The
[wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) pin the
line's key set, the default deserialization, and the plugin's log line; none
ties the TypeScript list to the Rust struct or states what a field brackets.

The key check runs under a pinned key-to-field map, which at HEAD is the
identity except `post_attach_ms` for `post_attach` (`transform.rs:1254`,
`:1331`, field at `:1162`). The bracket statement is a per-field table beside
the struct that a relocation changes in the same change; it is a review gate,
not a runtime assertion.

## Investigation log

### Q: Are the stage fields a contract with the plugin or free to change?

- Sources examined: [`rust-mode-transform.ts:1013-1042`][ts-stages], the
  plugin test at [`test.ts:249`][ts-test], `docs/host-wire-protocol.md` for
  `timings`.
- Findings: The plugin reads `handler_total`, `total`, and 22 stage keys,
  and the test asserts only that the `rust module stages:` line appears for
  responses carrying `handler_total`, `total`, and a native-cache count. The
  wire document does not mention `timings`.
- Missing evidence: A written statement of which consumer owns the field
  set.
- Conclusion: needs human input.

[pass-state-load]: https://github.com/ahrav/eidnara/blob/e451a2b4/crates/daemon/src/transform.rs#L1033-L1034
[tt]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1018-L1197
[rtcd]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1199-L1210
[fmt]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1216-L1349
[fmt-post-attach]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1254
[fmt-post-attach-value]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1331
[snap-add]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2373
[snap-once]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2852
[tc-local]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/token_cache.rs#L57-L76
[h-timings]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8463-L8488
[prepare]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4994-L5324
[respond]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L14404
[emit]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L14485-L14507
[ts-read]: https://github.com/ahrav/eidnara/blob/9132344/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L999-L1012
[ts-stages]: https://github.com/ahrav/eidnara/blob/9132344/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1013-L1042
[ts-stage-fn]: https://github.com/ahrav/eidnara/blob/9132344/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1019-L1024
[ts-test]: https://github.com/ahrav/eidnara/blob/9132344/packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L249

## Implementation evidence, 2026-09-13

[#438](https://github.com/ahrav/eidnara/issues/438) preserves timing field names
and units. It makes no stage-speedup claim and does not reactivate W2. The
following source observations distinguish placement from measurement.

| Field group | Boundary after relocation |
| --- | --- |
| `handler_total` | The pre-typed-decode `Instant` travels through `EntryTimings`; settlement reads its elapsed time. It includes unit admission queueing, blocking-pool scheduling, and Emergency95 waits before settlement. The permit precedes snapshot `begin` on both unit-backed lanes and unpaged typed acceptance, not this timer's start. Paged staging already accepts before typed admission; that behavior is unchanged. The timer ends inside the final unit, not after that unit's return hop, output reservation, or wire publication. |
| `pass_state_load` | Upstream's timer brackets preflight `store.load_meta` before delta expansion and unit admission. Its value travels through `EntryTimings` and is copied into the response at settlement. #438 preserves this parent field rather than introducing it. |
| `request_observed_to_handler`, `delta_expand` | The handler records these before unit submission and carries their values unchanged. |
| Projection lookup, side-channel drain, receive trace | The same operations have local `Instant` brackets inside `start_transform_pass`; unit queue time is not attributed to these stages. |
| Projection store, native attach, completion trace, response observation, retained size, snapshot store, `post_attach` | Settlement brackets the corresponding work on its executing thread. These fields are not redefined as the enclosing async wait. |
| Transform `total` and token-cache deltas | Each synchronous transform attempt keeps its own brackets and both thread-local samples on that attempt's blocking thread. Different attempts may use different threads; no delta subtracts one attempt's start from another's finish. |
| Trigger timings and native-cache counts | Preparation timings accumulate in `TransformedPass`; settlement copies them and the existing native-cache counts into the response. |

[Entry and submission][entry-live], [pre-transform brackets][pre-live], and
[settlement assignments][timings-live] establish these source boundaries.
`transform.rs` and `token_cache.rs` are unchanged by #438. The queued fifth-unit
test proves off-worker admission, not a measured queue-duration field or a
performance gain. The four-scalar Emergency95 assertion checks a read boundary,
not a latency improvement. No comprehensive per-field timing test or TypeScript-to-Rust
schema check is claimed, and no full-cap paged or slow-disk duration is measured.
The broader field-ownership question remains open on this invalidated record.

[entry-live]: ../../../../../crates/daemon/src/lib.rs#L8250-L8271
[pre-live]: ../../../../../crates/daemon/src/lib.rs#L8703-L8747
[timings-live]: ../../../../../crates/daemon/src/lib.rs#L8997-L9107

## Upstream stage evidence, e451a2b4

The upstream audit records a stage delta that is absent from the discovery
baseline: the [single pass-state load](consolidated-cache-state-reads-match-per-consumer-loads.md#single-load-evidence)
moves the `cache_state` read before `delta_expand` and `projection_cache_lookup`
and exposes its cost as [`pass_state_load`][pass-state-load]. Its updated
inventory has 90 struct fields, 91 printed keys, and 25 handler assignments,
including 14 duration fields. The key map retains the `post_attach_ms` exception.
This preserves the upstream timing correction, not a #438 speedup claim; W2
remains invalidated.

Upstream [loads metadata and records this duration][upstream-meta-load] before
constructing `PassState`, and [copies the field into the response][upstream-timing].
`pass_state_load_has_its_own_timing_bucket` supplies an upstream test for the
bucket. The rebased source retains that field and the metadata-load API:
[preflight][metadata-load-live] records the elapsed read, and
[settlement][timings-live] copies it. The table above includes this merged
boundary without turning it into a performance result.

[upstream-meta-load]: https://github.com/ahrav/eidnara/blob/e451a2b4/crates/daemon/src/lib.rs#L8257-L8267
[upstream-timing]: https://github.com/ahrav/eidnara/blob/e451a2b4/crates/daemon/src/lib.rs#L8799-L8809
[metadata-load-live]: ../../../../../crates/daemon/src/lib.rs#L8380-L8393
