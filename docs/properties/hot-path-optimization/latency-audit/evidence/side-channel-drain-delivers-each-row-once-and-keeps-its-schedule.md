# side-channel-drain-delivers-each-row-once-and-keeps-its-schedule

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

## Discovery trigger

The drain runs before every transform and costs at least one fenced
transaction (the leftover delete) even when the outbox is empty; a populated
drain costs two fenced transactions per row. The audit proposes skipping the
empty drain and deleting the row inside the delivery transaction. Both change
the drain's transaction shape. The row is the only duplicate guard for two of
the three target tables, and the drain's order, limit, and backoff decide which
rows a pass touches, so any restructuring must preserve those exactly.

## Evidence trail

- The handler calls [`drain_historian_side_channels`][drain-call] before the
  transform with [`HISTORIAN_SIDE_CHANNEL_DRAIN_PER_KIND`][kinds] (32) and
  discards the result.
- [`drain_historian_side_channels`][drain] first runs
  [`delete_delivered_historian_side_channels`][delete-all] in one fenced
  transaction, returns early when `per_kind_limit == 0`, then iterates
  [`HISTORIAN_SIDE_CHANNEL_KINDS`][kinds] (`event`, `primer`,
  `user_observation`); per kind it loads due rows with
  `per_kind_limit.min(32)`, delivers each, deletes on success, records a
  failure otherwise, and returns the first bookkeeping error after the loop.
  Its [doc][drain-doc] states the mark-then-delete intent.
- [`load_due_historian_side_channels`][load-due] selects
  `delivered_at_ms IS NULL AND next_attempt_at_ms <= ?3`, `INDEXED BY` the
  [order index][idx-order], `ORDER BY firing_seq, source_start, source_end,
  item_index LIMIT ?4`.
- [`deliver_historian_side_channel`][deliver] runs the target insert and
  [`mark_historian_side_channel_delivered_tx`][mark] in one `with_conn_fenced`
  call per kind; the mark updates under `delivered_at_ms IS NULL` and returns
  `QueryReturnedNoRows` when `changed != 1`, which rolls the transaction back.
  A test-only `fail_once` seam at [`:11234-11245`][fail-once] injects a
  failure per kind.
- [`record_historian_side_channel_failure`][failure] computes
  `delay = 1000 * 2^min(attempt_count, 6)` capped at
  [60,000][kinds], sets `next_attempt_at_ms = now + delay`, increments
  `attempt_count`, and stores the error truncated to 2,000 characters.
- [`delete_delivered_historian_side_channel`][delete-one] deletes the one row
  under `delivered_at_ms IS NOT NULL` in a second fenced transaction.
- [`historian_side_channel_status`][status-sc] counts pending rows as
  `delivered_at_ms IS NULL`.
- Targets: [`insert_historian_events_tx`][events-insert] and
  [`insert_historian_user_observation_tx`][obs-insert] are plain inserts;
  [`insert_historian_primer_tx`][primer-insert] upserts on
  `(project_path, harness, session_id, source_start_message_id,
  source_end_message_id)`.
- Rows are enqueued by [`publish_historian_chunk`][publish]; the outbox
  [primary key][outbox-sql] is `(session_id, firing_seq, kind, source_start,
  source_end, item_index)`. The publish task also drains after a committed
  publish ([`:11074-11083`][publish-drain]), so two drainers can overlap on one
  session.
- Firing needs a configured [`model_chain`][cfg-models] (the
  [`no_models` gate][no-models]); `user_observation` rows also need
  [`user_memory_collection_enabled`][cfg-user-mem], default `false`.

## Failure scenario

A crash between the mark commit and the delete leaves a marked row; the next
drain's leftover delete removes it, and `load_due` never selects it because it
filters on `delivered_at_ms IS NULL`. A design that deletes inside the delivery
transaction must make the delete affect exactly one row or roll back; a delete
that affects zero rows because a concurrent drainer already retired the row,
and still commits the target insert, duplicates a `compartment_events` or
`user_memory_candidates` row. An empty-drain shortcut that skips the leftover
delete leaves marked rows in the table and changes nothing the status counts,
but changes which rows a later pass touches. A reorder that changes kind
order, sort order, the limit, or the backoff constants changes which rows a
pass touches.

## Timing windows and dependencies

Two windows: between the mark commit and the per-row delete (crash), and
between one drainer's `load_due` and its `deliver` (a second drainer loads the
same row; the loser's mark returns `changed == 0`). The backoff depends on
`now_ms` supplied by the caller, so a test can place `now_ms` on either side of
`next_attempt_at_ms`.

## What a test must construct

A published firing with events, primers, and user observations; a crash or
abort injected between the two fenced transactions; a second drainer started
between load and deliver; multiple rows per kind across two firings; an
injected failure on one kind through the [`fail_once`][fail-once] seam;
`now_ms` before and after the computed `next_attempt_at_ms`. Assert exactly
one target row per outbox row across all drains, the kind and row order, the
limit, the backoff values, and the error cap. The
[state checks](../existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
cover restart redelivery ([`t-restart`][t-restart]), per-kind isolation
([`t-faults`][t-faults]), CAS-loser enqueue ([`t-publish-cas`][t-publish-cas]),
and revert deletion ([`t-truncate`][t-truncate]); none covers the crash
window, two drainers, ordering across firings, the limit, or the backoff.

## Investigation log

### Q: Does the drain doc match the code, and what must change with a fold?

- Sources examined: [`drain-doc`][drain-doc], [`deliver`][deliver],
  [`delete-one`][delete-one], [`status-sc`][status-sc].
- Findings: The doc says the acknowledgement row is deleted only after the
  delivery commit so restart replay is idempotent; the code matches. A
  delete-in-place design keeps replay idempotent by a different mechanism (the
  row is gone) and keeps the pending count unchanged because pending is
  `delivered_at_ms IS NULL`; the comment must change with it.
- Missing evidence: None for HEAD; this is documented intent versus a
  proposal, not a code disagreement.
- Conclusion: resolved with answer - the code and doc agree at HEAD; a fold
  must rewrite the doc and prove the one-row delete-or-rollback guard.

[drain-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8124-L8128
[no-models]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5189-L5196
[cfg-models]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/config.rs#L119
[cfg-user-mem]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/config.rs#L126
[kinds]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L4513-L4516
[publish]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10559
[publish-drain]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10762-L10771
[drain-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10803-L10805
[drain]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10806-L10851
[status-sc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10853-L10879
[load-due]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10881-L10915
[deliver]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10917-L10973
[fail-once]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10922-L10933
[failure]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10975-L11014
[delete-all]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L11016-L11029
[delete-one]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L11031-L11053
[mark]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L13739-L13764
[events-insert]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L13580-L13601
[primer-insert]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L13766-L13808
[obs-insert]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L13810-L13831
[t-faults]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L18704
[t-restart]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L18920
[t-publish-cas]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L19067
[t-truncate]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L20529
[outbox-sql]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/baseline.sql#L489-L505
[idx-order]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/baseline.sql#L531-L535

## Single-transaction evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/434)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

[Delivery][deliver-live] inserts the target row and [retires the outbox row][retire]
in one fenced transaction. The retirement is a `DELETE` whose
`delivered_at_ms IS NULL` predicate is the row-still-pending guard: a
drainer that read the row before another drainer retired it finds nothing to
delete, the delivery rolls its target insert back and reports the row as
already retired, and the drain counts it as neither delivered nor failed, so no
failure record or backoff is written for a row that no longer exists. The
predicate also names the payload: a handle read before a session reset cannot
consume a row re-issued under the same composite key with other bytes, as the
[key-reuse test][reuse-test] shows (the stale delivery reports already
retired, delivers nothing, and the re-created row stays pending). Drainers
on one store serialize on the connection, so the guard closes a stale read,
not two simultaneous statements. The per-row delete transaction is gone
because nothing remains for a restart or a concurrent drainer to redeliver: a
row is pending or absent. The [drain-start sweep][sweep] of rows an earlier
build marked delivered is retained, since such rows can exist in files written
before this change, but a read decides whether its fenced delete runs, so a
store without them pays no durable write. The schedule, the per-kind limit,
the ordering, and the backoff are untouched.

The [crash test][crash-test] injects a failure between the insert and the
retirement for every kind: the drain reports three failures, the target tables
hold nothing, and all three rows stay pending. That half shows the insert and
the retirement share one transaction; the baseline already held the insert and
the mark in one transaction, so it is not a behavior change. The gap the
baseline left, a crash between the mark commit and the delete commit, is
closed by construction because no second commit exists; the discriminating
assertion is that the outbox ends empty rather than marked. The test then
reads the due rows a second time before one drain delivers them, and each
stale delivery reports the row as already retired: each target holds one row
and the outbox holds none.

### Focused execution, 2026-09-12

`cargo test -p memory-store --locked` passed 180 tests including the test above
and the existing restart and per-kind isolation tests.

[deliver-live]: ../../../../../crates/memory-store/src/lib.rs#L11582-L11660
[retire]: ../../../../../crates/memory-store/src/lib.rs#L14425-L14450
[sweep]: ../../../../../crates/memory-store/src/lib.rs#L11706-L11730
[crash-test]: ../../../../../crates/memory-store/src/lib.rs#L20599-L20738
[reuse-test]: ../../../../../crates/memory-store/src/lib.rs#L17081-L17148
