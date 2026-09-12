# side-channel-row-is-due-during-a-drain

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

## Discovery trigger

The area's [portfolio evaluation](../portfolio-evaluation.md#gaps-queued)
queued gap 2: C3 is an explicit-config-only `always` record whose populated
path never runs on a default campaign, so its per-row clauses pass
vacuously. The disposition step found that the enabling state is
constructible from fixtures that already exist in `crates/memory-store` and
`crates/daemon`, so a `sometimes` witness is recorded instead of an
`always-or-unreached` reclassification.

## Evidence trail

- The handler drains on every pass at [`lib.rs:8124-8128`][pass-drain],
  passing `pass_now` and `HISTORIAN_SIDE_CHANNEL_DRAIN_PER_KIND`, and
  discards the result.
- [`drain_historian_side_channels`][drain] deletes delivered rows, then for
  each kind in [`HISTORIAN_SIDE_CHANNEL_KINDS`][kinds] loads due rows with
  [`load_due_historian_side_channels`][load-due], whose predicate is
  `delivered_at_ms IS NULL AND next_attempt_at_ms <= ?3`
  ([`:11205-11206`][due-predicate]). An empty outbox makes the loop a no-op.
- Rows enter the outbox inside the publish transaction
  ([`enqueue_historian_side_channels_tx`][enqueue]) and are drained inline
  right after the commit ([`:11074-11083`][publish-drain]), so on the happy
  path nothing is pending when the next pass drains.
- A failed delivery records `attempt_count + 1` and
  `next_attempt_at_ms = now + 1000 * 2^min(attempt, 6)` capped at 60 000 ms
  ([`:11293-11297`][backoff]); the first failure defers the row by 1000 ms.
- The publish itself needs a configured [`model_chain`][cfg-models]; user
  observation rows also need
  [`user_memory_collection_enabled`][cfg-user-mem]. Both default off, so a
  default campaign never enqueues a row.
- The seam [`fail_next_historian_side_channel_for_test`][fail-sc] inserts the
  kind into a set (`:5905-5908`), so three calls arm all three kinds for one
  publish. It is `#[cfg(any(test, feature = "test-support"))]`, and the
  daemon's dev-dependency on `memory-store` enables `test-support`
  ([`Cargo.toml:92`][daemon-cargo]).
- Two fixtures construct the state today.
  [`historian_side_channel_faults_are_isolated_and_retryable_per_kind`][t-faults-sc]
  publishes a firing carrying one event, one primer, and one user observation
  (`:18712-18750`) with one kind armed per iteration, asserts one pending
  row, then drains with `now_ms = i64::MAX` (`:18786-18788`).
  [`status_diagnostics_surface_pending_historian_side_channel_failure`][t-status-sc]
  arms `event`, publishes through a daemon handler's store, sleeps 1100 ms,
  and runs a transform pass whose drain delivers the row (`:35598-35604`).
- [`historian_side_channel_outbox_recovers_after_restart`][t-restart] shows
  the other route to a pending row: a failed inline delivery, then a store
  reopen, then a drain.

## Failure scenario

Not a violation; a coverage gap. A default campaign runs C3's drain on every
pass with an empty outbox. Every per-row clause (one delivery per row, the
same-transaction mark, the kind order, the `min(per_kind_limit, 32)` cap, the
backoff) is evaluated on zero rows, so a restructured drain that violates any
of them passes the campaign.

## Timing windows and dependencies

The due row must exist before the pass drain reads it and its
`next_attempt_at_ms` must be at or before the drain's `pass_now`. With one
failed inline delivery that is 1000 ms after the publish. The daemon test
sleeps 1100 ms for exactly this; a store-driven test passes `i64::MAX`.

## What a test must construct

A published firing with all three candidate kinds, on the shape of the
memory-store fixture; three `fail_next_historian_side_channel_for_test`
calls, one per kind, before the publish; then a pass at or past the backoff.
Before the drain delivers, read the outbox (or
[`historian_side_channel_status`][status-sc] for the count and a direct query
for the kinds) and record, under the three constant markers, that a row of
that kind is pending with `next_attempt_at_ms <= pass_now`. The markers
assert the input state; C3 asserts what the drain then does. No existing
test records the marker, and none has all three kinds due in one pass drain.

## Investigation log

### Q: Is the enabling state constructible from existing fixtures?

- Sources examined: [`fail_next_historian_side_channel_for_test`][fail-sc];
  the three tests named above; [`Cargo.toml:92`][daemon-cargo];
  [`publish_historian_chunk`][publish] and its inline drain.
- Findings: Yes. The seam is set-valued, the memory-store fixture already
  publishes all three kinds, and the daemon fixture already drives the pass
  drain against a pending row. Combining them needs no new seam.
- Missing evidence: None for constructibility; whether the campaign
  contract includes reaching the populated drain is the human decision the
  evaluation's bias 4 names, and this record answers it by supplying the
  witness rather than the exemption.
- Conclusion: resolved with answer - constructible; the witness is recorded.

[pass-drain]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8124-L8128
[t-status-sc]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L35555
[daemon-cargo]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/Cargo.toml#L92
[cfg-models]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/config.rs#L119
[cfg-user-mem]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/config.rs#L126
[kinds]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L4513-L4516
[fail-sc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L5900-L5909
[publish]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10559
[publish-drain]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10762-L10771
[drain]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10806-L10851
[status-sc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10853-L10879
[load-due]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10881-L10915
[due-predicate]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10893-L10894
[backoff]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10981-L10985
[enqueue]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L13713-L13737
[t-faults-sc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L18704-L18801
[t-restart]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L18920-L18974

## Marker evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/434)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

The [drain][drain-live] records a `DueSideChannelMarker` for every kind whose
due read returns rows: the kind, the pending rows read, and the drain's
`now_ms`, before delivery runs. The marker is compiled under `test-support`,
so daemon tests reach it; the store keeps the newest 64 so a long-lived
`test-support` daemon does not grow it without bound. The
[crash test][crash-test] asserts a marker for each of the three kinds with one
pending row; the situation is a failed inline delivery followed by a direct
call to the drain function, not a handler pass, so the witness is `reachable`
for the drain function rather than the `sometimes` witness the record's
guarantee names. A handler-pass run with all three kinds due is still open.

[drain-live]: ../../../../../crates/memory-store/src/lib.rs#L11456-L11514
[crash-test]: ../../../../../crates/memory-store/src/lib.rs#L20599-L20738
