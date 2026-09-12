# stage-timing-fields-keep-their-boundaries

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The per-stage timings are the only production numbers a latency change can
point to. The wildcard pass asked what protects the meaning of a field when
the work it brackets moves. It found that every field defaults to zero on
deserialization, that the handler-level pairs are taken on the handler task,
and that the token-cache delta is a thread-local subtraction, so a relocation
can shrink a number without making anything faster.

## Evidence trail

- [`TransformTimings`][tt] has 90 fields, each with `#[serde(default)]`, so
  a field removed from the producer deserializes as `0` or `0.0` on the
  consumer.
- [`format_pass_timing_line`][fmt] prints 91 `key=value` pairs (`session`
  plus one per field). The key for `post_attach` is `post_attach_ms`
  ([`:1264`][fmt-post-attach], value at [`:1342`][fmt-post-attach-value]);
  every other key equals its field name. The plugin's `rust module stages:`
  line ([`:1013-1042`][ts-stages]) reads 22 keys by name from the response
  object; all 22 exist in the struct. Its [`stage`][ts-stage-fn] helper
  prints `n/a` only when the key is absent or non-finite, so a present zero
  prints as `0.0`.
- The handler assigns 25 fields at [`:8549-8573`][h-timings]. Fourteen are
  millisecond durations from `Instant` pairs taken on the handler task:
  `handler_total` from `handler_started_at`, the request-to-handler gap,
  the pass-state load, delta expand, side-channel drain, receive trace, cache lookup and store,
  native attach, completion trace, observation, retained size, snapshot
  store, and `post_attach`. Six trigger fields come from
  `HistorianTriggerTimings` filled inside [`prepare_historian_fire`][prepare];
  five are native-cache counts.
- [`record_token_cache_delta`][rtcd] subtracts two reads of
  [`local_stats`][tc-local], a `thread_local!` counter whose doc says the
  counters exclude other threads and only differences are meaningful. The
  start read sits at [`apply_additive_only:2384`][snap-add] and
  [`apply_once:2863`][snap-once]; both functions are synchronous today.
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

## Recorded stage delta

The single pass-state load
([consolidated-cache-state-reads-match-per-consumer-loads](consolidated-cache-state-reads-match-per-consumer-loads.md#single-load-evidence))
moved the `cache_state` read from inside the `delta_expand` and
`projection_cache_lookup` windows to before both. It added
[`pass_state_load`][pass-state-load] as the read's own field and key instead
of letting those two buckets shrink by the read's cost, so no existing field
changed what it brackets and the key-to-field map stays the identity except
`post_attach_ms`. The counts above are the post-change counts.

## What a test must construct

An ordinary pass that populates `timings`, then a source-level assertion that
the plugin's 22 keys and the line's 91 keys resolve to struct fields (the
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

[tt]: ../../../../../crates/daemon/src/transform.rs#L1026-L1207
[rtcd]: ../../../../../crates/daemon/src/transform.rs#L1209-L1220
[fmt]: ../../../../../crates/daemon/src/transform.rs#L1226-L1360
[fmt-post-attach]: ../../../../../crates/daemon/src/transform.rs#L1264
[fmt-post-attach-value]: ../../../../../crates/daemon/src/transform.rs#L1342
[snap-add]: ../../../../../crates/daemon/src/transform.rs#L2384
[snap-once]: ../../../../../crates/daemon/src/transform.rs#L2863
[tc-local]: ../../../../../crates/daemon/src/token_cache.rs#L57-L76
[h-timings]: ../../../../../crates/daemon/src/lib.rs#L8549-L8573
[prepare]: ../../../../../crates/daemon/src/lib.rs#L5052-L5382
[respond]: ../../../../../crates/daemon/src/lib.rs#L14501
[emit]: ../../../../../crates/daemon/src/lib.rs#L14582-L14604
[pass-state-load]: ../../../../../crates/daemon/src/transform.rs#L1033-L1034
[ts-read]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L999-L1012
[ts-stages]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1013-L1042
[ts-stage-fn]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1019-L1024
[ts-test]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L249
