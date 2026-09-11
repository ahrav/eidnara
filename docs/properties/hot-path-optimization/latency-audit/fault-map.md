# Fault and enabling-state map for the latency audit supplement

The system is `/local/home/ahrav/scratch/eidnara`, at
`913234433ae36a80a6e22c6aac14c7f9aab74386`, checked on 2026-09-10.
The supplied audit and repository evidence define the
[pending-confirmation scope](catalog.md#scope-and-provenance). No external
plan or incident report is supplied. No fault campaign runs here. This is a
working-tree supplement, not part of the source baseline commit. The parent's
[fault map](../fault-map.md) remains authoritative for K, E, S, H, and R.

## Fault availability

| Class | Construction and availability | Limit |
| --- | --- | --- |
| Oversize and pool-short bodies | [`Handler::handle`][handle] takes the byte cap, footprint, and a non-awaiting scratch reservation on every request; a body over 32 MiB or a footprint over [`resident_capacity()`][capacity] refuses permanently, and concurrent parses can drain the shared [scratch pool][pools]. | `RequestCtx` is transport-private, so only a real host or a new seam drives the handler; the plugin [pages at 512 KiB][paging], so production oversize bodies have no known sender. |
| Discriminator and decode shapes | A body corpus reaches routing through [`dispatch_value_for_test`][testentry] and the page lane through [assembled pages][pageapply]. | serde duplicate-key behavior is cited from upstream, not exercised here. |
| Delta turns and cache hits | The plugin's delta protocol produces a projection cache hit and a reattached prefix on steady turns; [`expand_transform_tail_delta`][expand] is the seam. | A replayed unflagged synthetic pair needs a prior bust pass and a harness replay. |
| Mint commit failure | A CAS conflict on the tag mint commit leaves speculative rows uncommitted while the baseline entry is warm. | Requires a concurrent writer or an injected conflict at the commit. |
| Post-commit interleaving and abort | The `#[cfg(test)]` [`between_transform_and_prepare`][hook] runs between the transform and `prepare_historian_fire`. | The hook is test-only; the Emergency95 awaits at `:8263`, `:8289`, and `:8315` are the only production windows. |
| Trace and drain faults | An outbox delivery failure is injectable at the store (`fail_next_historian_side_channel_for_test`); two drainers exist in production (pass and publish task). | No seam injects a `pass_trace` upsert failure (the store's four `fail_next_*_for_test` seams at `memory-store/src/lib.rs:5900-5928` do not cover it); no fault point exists for a crash between the mark commit and the delete commit; SIGKILL at that point needs a child process. |
| SDK read failure and agent switch | An SDK fake can store a deny then reject or hang past [2000 ms][timeout]; `info.agent` changes per message. | Whether OpenCode emits a permission-change event is unresolved. |
| Second-session rows and read errors | Fixtures can seed two sessions in one database and make the file unreadable; `closeReadOnlySessionDb` is exported, and a test can rename a second database over the cached path. | OpenCode's real indexes are not in this repository; whether OpenCode ever replaces its database file in place is not stated here. |
| Byte-measure boundary | A lone surrogate in a string field takes the escape path; a body within a few bytes of 512 KiB under `JSON.stringify` may measure differently under `serde_json`. | Whether any real body crosses the boundary is unresolved. |
| Ring residency and direct frames | A released run of at least one batch on an idle ring, a wrapped run, and an aborted reservation are constructible in-crate; `publish_direct` accepts an injected serializer and deadline. | The direct path has no production sender; the [`direct_fill` fixture arm][fixture-arm] is the only entry. |
| CAS fault points and crashes | [`cas_fault_injection.rs`][t-faults] supplies six ingest and three GC fault points plus SIGKILL barriers. | GC and purge decrements have no daemon caller; a second ingest before orphan recovery is not constructed. |
| SOFT pressure | Memory update counts, m1 body size, and frozen m0 size are workload inputs under default configuration. | Boundary values need exact token counts recorded before the candidate runs. |
| Config and schedule edits | Tier files and the override can be rewritten between passes; two project roots can be bound; `Local` zone and DST are environment inputs. | The dreamer schedule defaults to `None`, so W6 is explicit-config-only. |
| Measurement identity | The host-runtime [`evidence.rs`][evidence] manifest discipline exists; production-sized fixtures exist as `#[ignore]` tests. | No daemon measurement records build, host, or workload identity. |
| Worker-thread panic | A panic injected inside a [`kernel_routes::blocking`][blocking] closure runs on a `spawn_blocking` worker whose redaction depth is `0`, so the hook forwards the full panic to the default hook; the host-runtime [child-process pattern][t-panic-child] captures stderr. | No daemon test panics inside a `blocking` closure; the injection point is a test-only branch or a test store that panics on a chosen call. |

## Required state per property

| Record | Required faults or enabling state | Independent observation |
| --- | --- | --- |
| [A1][a1] | Bodies over the cap, over the footprint ceiling, and under a drained pool; a large text block through the full typed decode. | Terminal code, ticket and route-channel absence, charge release, and retained copy count are read outside the handler. |
| [A2][a2] | The discriminator and shape corpus through both lanes and the probe. | The reference reader's route, admit decision, and typed result. |
| [A3][a3] | Two concurrent bodies whose footprints sum above the scratch reserve while each fits alone. | Footprint and pool availability at reservation entry. |
| [B1][b1] | A cache-hit delta turn with tool calls, repeated ids, an equal-but-not-pointer-equal chunk, and every served shell kind. | Full projection, full native encode, and `to_vec(to_value(msg))` computed independently. |
| [B2][b2] | The B5 situation: a replayed unflagged pair on a delta turn with a firing. | Synthetic sets at each observer and served bytes with and without the flag. |
| [B3][b3] | A warm baseline entry, a minting pass, a second pass; a mint commit that fails. | The store's ordered rows and the cache entry read directly. |
| [B4][b4] | A tail with every part kind and an excluded block, measured twice. | Independent `sha256(kind ++ NUL ++ content)` per part. |
| [B5][b5] | A prior bust pass, a harness replay without the flag, a delta body, a configured `model_chain`. | The three preconditions recorded at normalization and firing preparation. |
| [C1][c1] | A committing pass with a new no-fire reason; an Emergency95 pass with a publication in the window; a CAS conflict; rows with absent, unknown, `null`, and corrupt fields. | Post-commit `row_version` and a paired full `MemoryStore::load`. |
| [C2][c2] | Reject, stable, rerun, and fresh-session passes; an injected `pass_trace` failure beside a commit. | `pass_trace` counters and `cache_state.row_version` read after each pass. |
| [C3][c3] | A firing with all three kinds; a crash or abort between mark and delete; a second drainer between load and deliver; `now_ms` around the backoff. | Target table row counts and outbox state read directly. |
| [C4][c4] | Duplicate names, `BTreeMap`-key and integrity-field secrets, a container under a protected key, a clean `meta`. | Stored column bytes and the recorded scan compared with an independent walk. |
| [C5][c5] | A concurrent publish or recut through a second handle in the post-commit window. | Committed and observed `row_version` recorded separately. |
| [C6][c6] | A firing with all three kinds; a failed inline delivery per kind through the `test-support` seam, or a reopen between publish and drain; a pass at or past the 1000 ms backoff. | The outbox rows and their `next_attempt_at_ms` read before the drain delivers, against the drain's `now_ms`. |
| [P1][p1] | A stored deny then a failing read; two passes with different `info.agent`; a permission edit between passes. | The live evaluator's answer for the same inputs. |
| [P2][p2] | The fixture states, second-session rows, a read error, a connection replacement with cached statements, a file replaced at the same path while the connection is cached. | The frozen reference predicate as the oracle; the connection identity per statement; the file identity (`st_dev`, `st_ino`) before and after. |
| [P3][p3] | A lone-surrogate body, a paged body, and a boundary body with `f64` fields. | `writeUtf8`'s emitted count and the host's `serde_json` length. |
| [P4][p4] | Control characters and newlines in fields; a planted symlink; a foreign-uid directory. | The written file's bytes, mode, and the swallow counter. |
| [P5][p5] | An SDK fake answering `deny` once then failing. | Cache state at verdict entry and the read outcome. |
| [T1][t1] | Over-quotient `max_connections`; a released batch on an idle ring; an aborted reservation; a wrapped run. | Admission outcome; `arena_reclaimed - punched`; `mincore` residency of the aborted range. |
| [T2][t2] | A concurrent same-shape writer; mismatched span lengths; each word offset. | Byte-for-byte agreement of per-byte and bulk reads; no uninitialized byte in the returned `Vec`. |
| [T3][t3] | Underfill, overflow, error, and panic serializers; a wrapped body; an expired deadline; slow egress; generation retirement. | Ring frame visibility, header length, and the egress charge before and after commit. |
| [T4][t4] | Held egress ahead of a direct frame; a handler returning immediately. | Handler completion and commit as separate events. |
| [G1][g1] | A store at `cap - 1`; a re-ingested digest at the cap; retained invalidated bytes; an unrecovered orphan publish. | The independent walk and the absence of a reservation row after refusal. |
| [G2][g2] | The nine fault points; SIGKILL barriers; objects added or removed outside the store. | The independent `st_size` sum at every quiescent point. |
| [G3][g3] | `AfterEvents` after a publish; `INGEST_CRASH_POINT`; an aged invalidated reference; a purge; `ArtifactGcFault::Unlink`. | Usage before and after each path with a non-zero delta. |
| [W1][w1] | A production-sized session grown incrementally; an ingress body through the handler; warm and cold stores; a 64 MiB arena. | A recorded measurement naming build, host, workload, stage, and input. |
| [W2][w2] | A pass populating `timings` before and after a relocation. | Field presence in the struct and both consumers; the bracketed work per field. |
| [W3][w3] | Concurrent passes; 65_536 distinct digests; a minting pass; a SOFT threshold crossing. | Direct tokenizer counts, thread-local stats, and the declared-bytes sum. |
| [W4][w4] | A match far from its anchor; a match at an edge margin; a candidate-limit input; an over-`MAX_REDACTABLE_BYTES` input. | The whole-input scan's findings, limits, and digest. |
| [W5][w5] | A restart with an in-flight firing; a chunk over `token_budget`. | The frozen reference bytes and the fingerprint recomputed from lengths only. |
| [W6][w6] | A `Local` zone with DST; unsatisfiable and once-satisfiable expressions; `i64` extremes. | The minute stepper's `Option<i64>`. |
| [W7][w7] | Tier rewrites with later mtimes; an override edit; two bound project roots. | A fresh merge of the current file contents. |
| [W8][w8] | A committing pass with a pinned guidance date; an abort in the window (W11); a second pass. | The next pass's `ProducerContext` inputs and the projection cache read directly. |
| [W9][w9] | SOFT passes at update counts 40 and 41, at the 0.20 budget boundary, and at m0 499 and 500 with the 0.15 ratio boundary; a placeholder m1; no m0 unit. | Direct token counts on the frozen m0 and composed m1 recorded before the candidate runs. |
| [W10][w10] | More than 40 updates; an m1 over a fifth of a small budget; an m0 of at least 500 tokens with an m1 over 15 percent of it; a pass below all three. | The three inputs measured independently per pass. |
| [W11][w11] | The interleave hook or its relocation-era equivalent triggering an abort. | Commit through `row_version` and abort ordering observed separately. |
| [W12][w12] | A sentinel-bearing panic inside a `kernel_routes::blocking` closure or the relocated transform, in a child process; the same panic on the runtime worker as the control. | The child's stderr bytes and the terminal frame, read by the parent process. |
| [W13][w13] | A `ScriptedHost` project with a valid cron in `MODULE` authority and a `ManualClock` advanced past the instant; or a user tier with `dreamer_review_user_memories_schedule` set and a bound `MODULE` route. | The project, schedule, `now_ms`, and finite instant recorded at the `next_due` call. |

## Coverage checks to add

A3, B5, C5, C6, G3, P5, T4, W1, W10, W11, and W13 use their slugs as
constant, globally unique marker names. W10 uses four constant markers
suffixed `-updates`, `-budget-share`, `-m0-ratio`, and `-below`; C6 uses
three suffixed `-event`, `-primer`, and `-user-observation`; G3 uses five
suffixed `-cleanup`, `-recovery`, `-reclaim`, `-purge`, and `-retry`; W1
uses one per stage the specification enumerates. All are named in full in
their records; none is constructed dynamically. Each marker asserts input or
scheduling preconditions, never the defect:

- A3 asserts footprint fit and pool shortfall, not the handler's code or side
  effects.
- B5 asserts delta, the replayed-id input, and firing, not observer
  agreement.
- C5 asserts the foreign commit's `row_version` and its ordering before the
  pass's post-commit read, not a stale consumer.
- C6 asserts a due outbox row per kind at the pass drain, not the delivery.
- G3 asserts a non-zero independent byte delta per path, not counter
  agreement.
- P5 asserts a cached deny and a failed read, not the served verdict.
- T4 asserts queueing, handler completion, and uncommitted publication, not
  the charge accounting.
- W1 asserts a recorded run at production shape per stage, not a speed
  result.
- W10 asserts the three predicate inputs, not `pressure_refold`.
- W11 asserts commit then abort ordering, not the next pass's state.
- W13 asserts a configured schedule reached `next_due` with a finite
  instant, not the instant's correctness or the slot's run.

No `sometimes` marker here is paired with an `always(!X)` on the same
predicate. B2, C1, C3, T3, W6, W8, and W9 are the `always` records the
markers support; each asserts its own condition on every evaluated pass.

Other records need explicit case accounting: discriminator shapes and both
lanes for A2; the three artifact families for B1; the four observers for B2;
reject, stable, rerun, and fresh passes for C2; kinds, order, limit, and
backoff values for C3; policies and key positions for C4; agent and
permission inputs for P1; the fixture states plus second-session rows for
P2; the three measures for P3; the nine fault points for G2; every
`TransformTimings` field for W2; the three direct call sites for W3; rule
classes and limit modes for W4; the DST cases for W6; and the three per-pass
callers for W7. Missing case accounting remains missing evidence even if
assertions pass.

## Cheapest valid oracle first

1. A2, B4, C4, P2, P3, W5, W6, and W9 have a pure-function reference at HEAD
   (the current reader, digest, stepper, predicate, or frozen differential).
   Start there with corpora and boundary values, not a new harness.
2. B1, B3, C1, C2, C6, W3, W7, and W13 need one process and a real store or
   file system but no concurrency: run both differential gates from an
   integration test, fail a mint commit, pair a narrow read with a full
   load, inject a trace failure, edit tiers between passes, arm the three
   side-channel kinds before a publish, and tick a scripted scheduler past a
   cron instant.
3. A1, A3, P1, P5, T1, T2, and W4 need a driven host, an SDK fake, an
   in-crate ring, or a bounded-scan implementation to compare against; the
   ring and lease tests already run under Miri and Valgrind. W12 needs a
   child process on the host-runtime stderr-capture pattern.
4. C3, C5, G1, G2, G3, T3, T4, W8, W10, and W11 need crash points, a second
   drainer or writer, held egress, or an abort in a window that exists at
   HEAD only through the test hook or the Emergency95 awaits. Resolve
   execution topology before selecting a timing harness for T3, T4, W8, and
   W11.
5. W1 and W2 constrain the measurement itself and gate every "faster" claim
   in this area; they cost a manifest and a size-class decision, not a test
   harness.

This ranking routes work to `/testing:test-strategy`; it does not choose
final test forms or set a benchmark acceptance policy. No new global duration
or per-job bound is invented. The direct path's failure classification, the
deferred-punch bound, the durable-counter reconciliation action, the SOFT
constants' status, and the plugin fail-open default remain owner gates named
in the records' open questions.

[handle]: ../../../../crates/daemon/src/lib.rs#L11805-L11827
[capacity]: ../../../../crates/host-runtime/src/handler.rs#L486-L491
[pools]: ../../../../crates/host-runtime/src/runtime.rs#L814-L822
[paging]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
[testentry]: ../../../../crates/daemon/src/lib.rs#L12484-L12499
[pageapply]: ../../../../crates/daemon/src/lib.rs#L9425-L9433
[expand]: ../../../../crates/daemon/src/lib.rs#L4151-L4245
[hook]: ../../../../crates/daemon/src/lib.rs#L8224-L8232
[timeout]: ../../../../packages/opencode-plugin/src/shared/with-timeout.ts#L2
[fixture-arm]: ../../../../crates/host-runtime/tests/support/mod.rs#L441-L455
[t-faults]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[evidence]: ../../../../crates/host-runtime/benches/support/evidence.rs#L1-L8
[blocking]: ../../../../crates/daemon/src/kernel_routes/mod.rs#L462-L468
[t-panic-child]: ../../../../crates/host-runtime/tests/dispatch.rs#L603-L660
[a1]: catalog.md#admission-chain-charges-before-decode-and-refuses-effect-free
[a2]: catalog.md#route-and-typed-decode-are-independent-of-entry-path
[a3]: catalog.md#scratch-pool-shortfall-reaches-the-parse-reservation
[b1]: catalog.md#derived-artifacts-are-ownership-independent
[b2]: catalog.md#synthetic-normalization-is-scoped-to-the-pass
[b3]: catalog.md#tag-baseline-cache-entry-is-never-mutated-by-a-pass
[b4]: catalog.md#hygiene-digest-is-kind-prefixed-part-content
[b5]: catalog.md#replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn
[c1]: catalog.md#consolidated-cache-state-reads-match-per-consumer-loads
[c2]: catalog.md#pass-trace-writes-count-every-pass-outside-the-cache-cas
[c3]: catalog.md#side-channel-drain-delivers-each-row-once-and-keeps-its-schedule
[c4]: catalog.md#meta-json-preparation-scans-every-persisted-byte
[c5]: catalog.md#foreign-write-lands-between-pass-loads
[c6]: catalog.md#side-channel-row-is-due-during-a-drain
[p1]: catalog.md#cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs
[p2]: catalog.md#mid-turn-read-is-invariant-under-query-collapse-and-statement-caching
[p3]: catalog.md#paged-body-measure-equals-declared-frame-length-and-fits-host-caps
[p4]: catalog.md#log-lines-keep-sanitizer-and-file-hardening-guarantees
[p5]: catalog.md#todowrite-deny-then-read-failure-is-exercised
[t1]: catalog.md#arena-residency-is-bounded-by-admission-and-one-punch-batch
[t2]: catalog.md#arena-payload-copies-keep-the-address-derived-atomic-shape
[t3]: catalog.md#direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges
[t4]: catalog.md#direct-frame-outlives-its-handler-before-publication
[g1]: catalog.md#artifact-admission-fails-closed-against-on-disk-object-bytes
[g2]: catalog.md#reported-artifact-usage-equals-on-disk-object-bytes-after-recovery
[g3]: catalog.md#artifact-byte-decrement-paths-are-exercised
[w1]: catalog.md#optimized-stage-is-measured-at-production-shape
[w2]: catalog.md#stage-timing-fields-keep-their-boundaries
[w3]: catalog.md#token-cache-is-a-pure-declared-memo-behind-one-estimator-interface
[w4]: catalog.md#bounded-secret-scan-finds-every-whole-input-finding
[w5]: catalog.md#historian-firing-input-is-preserved-by-cheaper-construction
[w6]: catalog.md#cron-next-occurrence-matches-the-minute-stepper
[w7]: catalog.md#effective-config-reads-observe-a-tier-change-by-the-next-pass
[w8]: catalog.md#committed-transform-bookkeeping-is-applied-or-recomputed
[w9]: catalog.md#soft-pressure-refold-predicate-preserves-its-classification
[w10]: catalog.md#soft-pressure-refold-thresholds-are-each-crossed
[w11]: catalog.md#abort-lands-between-transform-commit-and-bookkeeping
[w12]: catalog.md#worker-thread-panics-stay-inside-the-redaction-boundary
[w13]: catalog.md#cron-schedule-is-evaluated-for-a-configured-project
