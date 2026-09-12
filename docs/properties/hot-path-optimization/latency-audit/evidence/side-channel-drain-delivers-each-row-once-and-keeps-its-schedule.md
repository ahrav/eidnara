# side-channel-drain-delivers-each-row-once-and-keeps-its-schedule

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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

[drain-call]: ../../../../../crates/daemon/src/lib.rs#L8199-L8203
[no-models]: ../../../../../crates/daemon/src/lib.rs#L5238-L5245
[cfg-models]: ../../../../../crates/daemon/src/config.rs#L119
[cfg-user-mem]: ../../../../../crates/daemon/src/config.rs#L126
[kinds]: ../../../../../crates/memory-store/src/lib.rs#L4635-L4638
[publish]: ../../../../../crates/memory-store/src/lib.rs#L10871
[publish-drain]: ../../../../../crates/memory-store/src/lib.rs#L11074-L11083
[drain-doc]: ../../../../../crates/memory-store/src/lib.rs#L11115-L11117
[drain]: ../../../../../crates/memory-store/src/lib.rs#L11118-L11163
[status-sc]: ../../../../../crates/memory-store/src/lib.rs#L11165-L11191
[load-due]: ../../../../../crates/memory-store/src/lib.rs#L11193-L11227
[deliver]: ../../../../../crates/memory-store/src/lib.rs#L11229-L11285
[fail-once]: ../../../../../crates/memory-store/src/lib.rs#L11234-L11245
[failure]: ../../../../../crates/memory-store/src/lib.rs#L11287-L11326
[delete-all]: ../../../../../crates/memory-store/src/lib.rs#L11328-L11341
[delete-one]: ../../../../../crates/memory-store/src/lib.rs#L11343-L11365
[events-insert]: ../../../../../crates/memory-store/src/lib.rs#L13892-L13913
[mark]: ../../../../../crates/memory-store/src/lib.rs#L14051-L14076
[primer-insert]: ../../../../../crates/memory-store/src/lib.rs#L14078-L14120
[obs-insert]: ../../../../../crates/memory-store/src/lib.rs#L14122-L14143
[t-faults]: ../../../../../crates/memory-store/src/lib.rs#L19314
[t-restart]: ../../../../../crates/memory-store/src/lib.rs#L19519
[t-publish-cas]: ../../../../../crates/memory-store/src/lib.rs#L19637
[t-truncate]: ../../../../../crates/memory-store/src/lib.rs#L21099
[outbox-sql]: ../../../../../crates/memory-store/baseline.sql#L489-L505
[idx-order]: ../../../../../crates/memory-store/baseline.sql#L531-L535
