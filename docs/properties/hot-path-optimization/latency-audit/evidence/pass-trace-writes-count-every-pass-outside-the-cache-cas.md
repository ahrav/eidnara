# pass-trace-writes-count-every-pass-outside-the-cache-cas

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

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
[received-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8131
[rejected-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8194-L8201
[rerun-a]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8264-L8267
[rerun-b]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8372-L8375
[completed-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8436
[status-read]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L6197-L6247
[age]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L6238
[health-read]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7826-L7874
[t-reject]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L23449
[t-success]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L23479
[t-repeat]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L23495
[t-frozen]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L23523
[t-status]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L23556
[stable-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1819-L1843
[t-sched]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L13524
[sched-test]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L13567
[passtrace-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L767-L784
[received-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6482-L6484
[received]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6485-L6535
[flagged]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6496-L6514
[stable]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6540-L6632
[stable-ring]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6594-L6601
[completed-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6634-L6636
[completed]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6637-L6685
[rejected-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6687-L6690
[rejected]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6691-L6744
[reject-bump]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6738
[sched-history]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6792-L6825
[commit-trace]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L8427-L8494
[commit-init]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L8441
[t-secret]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L15376
[t-upserts]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L17545
[plugin]: ../../../../../packages/opencode-plugin/src/hooks/context/command-handler.ts#L265-L268

## Receive-cost evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/434)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

The receive write stays outside the cache compare-and-swap; the specification
excludes folding it in because the counter increment is not idempotent. Its
cost falls two ways. Audit identifiers are [generated in Rust][opaque-id] from
the workspace random source, so an audit write spends no statement per
identifier; the per-row link copy keeps SQLite's `randomblob` because it needs
one value per selected row. The receive write [opts in][receive-opt-in] to
[skipping its audit rows][audit-skip] when its one scan preserved an existing
identity and found nothing: it substituted no byte, refused nothing, and
carries no detection. The opt-in is per write. The completed-trace, authority,
lineage, and compartment writes keep recording their clean scans, so the R1
parity check and the redaction receipts other tests count are unchanged. Any
substituting, rejecting, or detecting scan keeps the receive write's rows, and
the [receive test][receive-test] shows a clean receive leaving the audit
tables alone while a detected identity on a known session still records its
row. The skip applies only to a `session_id` that `pass_trace` or `cache_state`
already holds, decided by [one point lookup][receive-known] inside the
transaction: the receive that introduces a session is the only durable write
for that identity until the pass commits or rejects, so it keeps its
zero-finding receipt, as the [first-receive test][first-receive-test] shows.
The identifiers are generated inside the fenced transaction, so a random
source failure now aborts the write where `randomblob` could not fail; on
Linux after boot that failure is not reachable.

The scans a pass records for the identity, `core_state`, and `meta` bytes the
next pass replaces are owned by [one fixed pass owner][pass-owner] that the
next pass [retires after every replay check has passed][retire]; the key is
not per row version because other writers (historian publish, lineage descent,
recomputation reset) bump `row_version` without registering an owner, and a
key they never wrote could not be retired. Every scan for bytes that outlive
the pass is [reassigned][overlay-owner] to a [retained owner][retained-owner]
the pass never retires: the tag, temporal-mark, user-hint, and channel-1
overlay rows, the root added to `transform_session_roots`, the
`scheduler_observation` and `scheduler_interesting` entries appended to the
`pass_trace` history rings, and a `first_divergence` that stays readable as
`last_divergence` after a pass with none. Reassignment leaves the write's
default owner list alone, so a scan prepared after it keeps the pass owner;
the [reassignment test][reassign-test] holds that. The retained owner is
registered only when the pass carries such a scan, and its key differs from
the `cache_state` key that stores written before the pass owner existed carry,
so legacy per-pass rows on those stores stay separable from live ones. Those
legacy rows are not retired by this change; they stop growing, and a targeted
cleanup remains open.

The retirement adds a fixed number of cached statements to the fenced commit
(one scope lookup, one retired-scan select, one owner delete, one
[set-based orphan-scan prune and one batch prune][prune] over the retired ids
bound as a JSON array, and a scope prune) that the receive-side saving does
not offset; the branch's stated cost claim is about the receive write, and the
retirement is what keeps the pass-owned audit rows bounded. The retained
owner's rows grow with the bytes they describe: one receipt per stored root,
per ring entry, and one for the divergence readable as `last_divergence`. The
history rings keep 256 entries, so a pass that appends to a full ring
[evicts the oldest retained receipt][ring-evict] for that field in the same
transaction, and a new divergence evicts the receipt of the one it replaces;
the [ring test][ring-test] shows the receipt count for each ring field equal
to its ring length across 296 passes. A root the session already stores
[keeps its earlier receipt][root-stored]: the re-observing pass's scan stays
under the pass owner and is retired by the next pass, so the
[root test][root-test] shows one retained root receipt and one divergence
receipt across five passes that repeat both. A `scheduler_full_array_fingerprint`
is stored only inside a `scheduler_interesting` entry, so its scan
[joins the retained owner][fingerprint-retained] only when that entry is
written and is evicted with the interesting ring; the
[fingerprint test][fingerprint-test] shows five passes with a fingerprint and
no interesting entry holding no retained fingerprint receipt. The receipts are counts per
field, not links to individual ring entries: an entry appended by
`trace_pass_stable` carries a `pass_trace`-owned receipt, and evicting it
from the ring removes the oldest retained receipt instead, so the retained
count stays bounded by the ring length while the pairing of receipt to entry
is not recorded in either design. Lineage descent copies the source scope's live scans, which after
retirement are the latest pass's scans plus the retained scans rather than
every pass the source ever ran. The [retirement test][retire-test] shows the
`field_scans` and `scan_owner_copies` counts flat across six passes, flat
again across passes after a historian publish bumped the row version, a tag
mint's scans added and kept through the next pass, and the pass owner, the
retained owner, and the publish owner each holding exactly their own copies.
The [retained-fields test][retained-test] shows a second pass keeping the
receipts for the first pass's root, scheduler observation, interesting
observation, and divergence while both roots, both history entries, and the
divergence stay stored, and a third pass adding to them. The
[conflict test][seq-conflict-test] shows a pass that loses the
compartment-generation check retiring nothing.

The [daemon test][outcome-test] shows a rejected, a committed, and a stable
pass counting three receives, and a fourth pass whose receive UPSERT is
[injected to fail inside its own transaction][receive-fail] still committing
its cache state.

### Focused execution, 2026-09-12

`cargo test -p memory-store --locked` passed 183 tests including the six
above; `cargo test -p daemon --locked` passed 1022 unit tests, and the
`embedding_dispatch` integration test
`publication_search_deadline_preserves_admission_without_recharging` fails when
its file runs as a group and passes in isolation, on the unmodified branch head
as well.

[opaque-id]: ../../../../../crates/memory-store/src/lib.rs#L2629-L2637
[audit-skip]: ../../../../../crates/memory-store/src/lib.rs#L2444
[receive-test]: ../../../../../crates/memory-store/src/lib.rs#L16760-L16802
[receive-opt-in]: ../../../../../crates/memory-store/src/lib.rs#L7036
[receive-known]: ../../../../../crates/memory-store/src/lib.rs#L7066-L7078
[first-receive-test]: ../../../../../crates/memory-store/src/lib.rs#L16738-L16755
[seq-conflict-test]: ../../../../../crates/memory-store/src/lib.rs#L16704-L16733
[pass-owner]: ../../../../../crates/memory-store/src/lib.rs#L2863
[retained-owner]: ../../../../../crates/memory-store/src/lib.rs#L2866
[retire]: ../../../../../crates/memory-store/src/lib.rs#L9011-L9017
[prune]: ../../../../../crates/memory-store/src/lib.rs#L2644-L2686
[overlay-owner]: ../../../../../crates/memory-store/src/lib.rs#L8962-L8964
[retire-test]: ../../../../../crates/memory-store/src/lib.rs#L16561-L16699
[retained-test]: ../../../../../crates/memory-store/src/lib.rs#L16841-L16940
[ring-evict]: ../../../../../crates/memory-store/src/lib.rs#L9100-L9124
[ring-test]: ../../../../../crates/memory-store/src/lib.rs#L17153-L17216
[root-stored]: ../../../../../crates/memory-store/src/lib.rs#L9126-L9139
[root-test]: ../../../../../crates/memory-store/src/lib.rs#L16945-L16990
[fingerprint-retained]: ../../../../../crates/memory-store/src/lib.rs#L8944-L8951
[fingerprint-test]: ../../../../../crates/memory-store/src/lib.rs#L16995-L17042
[reassign-test]: ../../../../../crates/memory-store/src/lib.rs#L24441-L24464
[outcome-test]: ../../../../../crates/daemon/src/lib.rs#L24789-L24833
[receive-fail]: ../../../../../crates/memory-store/src/lib.rs#L7045
