# pass-trace-writes-count-every-pass-outside-the-cache-cas

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit proposes folding the `trace_pass_received` write into the
`commit_transform` transaction to save one fenced, fsynced transaction per
pass. The receive breadcrumb runs before the transform and counts every pass
regardless of outcome; the commit runs only when the transform commits. A fold
changes which passes are counted and couples a diagnostic write to the
cache-state CAS, which two doc comments state it must not do. The parent's
[R3][r3] owns the owner-relationship question the fold raises.

## Evidence trail

- The handler calls [`trace_pass_received`][received-call] before
  `run_transform`, [`trace_pass_rejected`][rejected-call] inside the reject
  closure, and [`trace_pass_completed`][completed-call] after the native
  attach; the transform calls [`trace_pass_stable`][stable-call] only when
  `!pass.response.committed`. Every call discards its result with `let _ =`.
- [`trace_pass_received`][received] is one UPSERT that inserts
  `receive_count = 1, first_divergence = NULL` or updates
  `receive_count + 1` and `first_divergence = NULL`; its [doc][received-doc]
  says it stays outside the fenced cache-state transaction so the write never
  contends with or extends the pass commit. A secret-bearing `session_id` is
  [tolerated only when the row already exists][flagged].
- [`trace_pass_rejected`][rejected] updates `reject_count + 1`
  ([`:6738`][reject-bump]); its [doc][rejected-doc] calls it a single plain
  UPSERT outside the fenced transaction. [`trace_pass_completed`][completed]'s
  [doc][completed-doc] says it cannot alter CAS semantics or hold the commit
  transaction open longer.
- [`trace_pass_stable`][stable] appends one `scheduler_history` observation
  with the 256-entry ring ([`:6594-6601`][stable-ring]); the
  [in-commit upsert][commit-trace] does the same, initializes `receive_count`
  to `0` on a fresh insert ([`:8441`][commit-init]), and leaves it alone on
  conflict.
- The [`PassTrace` doc][passtrace-doc] says the counters are stored apart from
  `cache_state` so a rejected pass leaves a trail without advancing
  `row_version`.
- Readers: [session status][status-read] and [health][health-read] JSON, the
  `newest_pass_at` age at [`:6238`][age], and the plugin's
  [`Passes: N received, M rejected`][plugin] line.
  [`load_pass_scheduler_history`][sched-history] has one non-store caller,
  a [test][sched-test].
- An Emergency95 pass can rerun `run_transform` at [`:8264-8267`][rerun-a] and
  [`:8372-8375`][rerun-b] after one receive breadcrumb.

## Failure scenario

A fold moves the bump into `commit_transform`. A rejected pass runs no commit,
so `receive_count` stops increasing and the existing tests' invariant
`receive_count == reject_count` after rejects breaks; a stable pass has no
commit transaction either. Attaching the bump to every commit double-counts an
Emergency95 rerun. Inside the fused transaction a `pass_trace` constraint
failure or the `session_id` identity refusal rolls back the cache-state row,
so a diagnostic vetoes a state commit.

## Timing windows and dependencies

The receive write precedes the outcome, so `last_received_at_ms` is set before
the transform runs; a fold changes that ordering for every pass. The identity
refusal is reachable at HEAD for a secret-bearing `session_id` on a known
session. No interleaving is required.

## What a test must construct

A transform that rejects (ordinal violation); a stable pass; an Emergency95
pass that reruns and commits twice; a fresh session whose first pass commits;
an injected failure in the `pass_trace` upsert during a committing pass.
Assert `receive_count` increases by one per request and `last_received_at_ms`
is set before the outcome; `reject_count` increases by one with `row_version`
unchanged; `first_divergence` is NULL after a reject; `scheduler_history`
gains one observation per accepted pass; and the cache-state row commits when
the trace write fails. The
[state checks](../existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
list seven pass-trace tests ([`t-reject`][t-reject], [`t-success`][t-success],
[`t-repeat`][t-repeat], [`t-frozen`][t-frozen], [`t-status`][t-status],
[`t-upserts`][t-upserts], [`t-sched`][t-sched]) and the identity gate
([`t-secret`][t-secret]); none covers `first_divergence` after a reject,
`receive_count` after a rerun, or a trace failure beside a commit.

Add a CAS conflict on the first commit attempt so the retry loop
(`transform.rs:1940-1979`) reruns `apply_once` and commits once, and assert
one breadcrumb, not zero or two. No store seam injects the `pass_trace`
failure at HEAD: the four `fail_next_*_for_test` seams
(`memory-store/src/lib.rs:5900`, `:5914`, `:5921`, `:5928`) cover the side
channel, the authority route read, and dreamer tasks only.

## Investigation log

### Q: Is under-counting rejected passes an acceptable semantic change?

- Sources examined: [`trace_pass_received`][received], the
  [`PassTrace` doc][passtrace-doc], [`t-reject`][t-reject],
  [`t-frozen`][t-frozen].
- Findings: The doc states the reject-trail purpose; the tests encode
  `receive_count == reject_count` after rejects. A fold cannot preserve that
  without a second write on the reject path.
- Missing evidence: A decision on the counter's meaning.
- Conclusion: needs human input.

### Q: If a fold is accepted, which doc comment is rewritten?

- Sources examined: [`received-doc`][received-doc],
  [`completed-doc`][completed-doc], [`rejected-doc`][rejected-doc].
- Findings: Three comments make the "outside the fenced transaction" promise;
  folding one of three writes does not remove per-pass trace transactions.
- Missing evidence: The replacement promise.
- Conclusion: needs human input.

### Q: Does moving the `session_id` scan under `cache_state` change ownership?

- Sources examined:
  [`write.domain_owner("session", .., "pass_trace")`][received] at `:6491`,
  [R3][r3].
- Findings: The trace write records its scan under the `pass_trace` owner in
  the `TransformDiagnostics` family; a fold moves it under the commit's owner.
- Missing evidence: R3's normalization decision.
- Conclusion: unresolved, needs R3's owner-relationship normalization.

[r3]: ../../catalog.md#redaction-audit-does-not-depend-on-retained-payload
[received-call]: ../../../../../crates/daemon/src/lib.rs#L8131
[rejected-call]: ../../../../../crates/daemon/src/lib.rs#L8194-L8201
[rerun-a]: ../../../../../crates/daemon/src/lib.rs#L8264-L8267
[rerun-b]: ../../../../../crates/daemon/src/lib.rs#L8372-L8375
[completed-call]: ../../../../../crates/daemon/src/lib.rs#L8436
[status-read]: ../../../../../crates/daemon/src/lib.rs#L6197-L6247
[age]: ../../../../../crates/daemon/src/lib.rs#L6238
[health-read]: ../../../../../crates/daemon/src/lib.rs#L7826-L7874
[t-reject]: ../../../../../crates/daemon/src/lib.rs#L23458
[t-success]: ../../../../../crates/daemon/src/lib.rs#L23488
[t-repeat]: ../../../../../crates/daemon/src/lib.rs#L23504
[t-frozen]: ../../../../../crates/daemon/src/lib.rs#L23532
[t-status]: ../../../../../crates/daemon/src/lib.rs#L23565
[stable-call]: ../../../../../crates/daemon/src/transform.rs#L1819-L1843
[t-sched]: ../../../../../crates/daemon/src/transform.rs#L13524
[sched-test]: ../../../../../crates/daemon/src/transform.rs#L13567
[passtrace-doc]: ../../../../../crates/memory-store/src/lib.rs#L767-L784
[received-doc]: ../../../../../crates/memory-store/src/lib.rs#L6482-L6484
[received]: ../../../../../crates/memory-store/src/lib.rs#L6485-L6535
[flagged]: ../../../../../crates/memory-store/src/lib.rs#L6496-L6514
[stable]: ../../../../../crates/memory-store/src/lib.rs#L6540-L6632
[stable-ring]: ../../../../../crates/memory-store/src/lib.rs#L6594-L6601
[completed-doc]: ../../../../../crates/memory-store/src/lib.rs#L6634-L6636
[completed]: ../../../../../crates/memory-store/src/lib.rs#L6637-L6685
[rejected-doc]: ../../../../../crates/memory-store/src/lib.rs#L6687-L6690
[rejected]: ../../../../../crates/memory-store/src/lib.rs#L6691-L6744
[reject-bump]: ../../../../../crates/memory-store/src/lib.rs#L6738
[sched-history]: ../../../../../crates/memory-store/src/lib.rs#L6792-L6825
[commit-trace]: ../../../../../crates/memory-store/src/lib.rs#L8427-L8494
[commit-init]: ../../../../../crates/memory-store/src/lib.rs#L8441
[t-secret]: ../../../../../crates/memory-store/src/lib.rs#L15376
[t-upserts]: ../../../../../crates/memory-store/src/lib.rs#L17545
[plugin]: ../../../../../packages/opencode-plugin/src/hooks/context/command-handler.ts#L265-L268
