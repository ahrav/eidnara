# lease-saturation-is-reached-then-drains

## Citation refresh, 2026-08-31 (eventfd rewrite)

PR #131 (merge `5d638e3e8`) replaced the polling wake mechanism with sparse
eventfd doorbells and moved every cited `ring.rs` line. All citations below
were re-verified against HEAD. The situation and the marker are unchanged; the
drain half gains a wake step, noted where it matters.

## Discovery trigger

`receive-resumes-when-lease-capacity-clears` is only meaningful if the lease set ever
actually fills while frames are waiting. In the shipped host that never happens: the
endpoint loop holds at most one of eight leases and releases it before returning. So a
recovery property written against lease saturation would pass forever in the
configuration that ships, testing nothing. This record makes the precondition a
first-class obligation instead of an assumption buried in another record's enabling
state.

## Evidence trail

- The situation has two independent halves, and the first gate reads only the first of
  them. `crates/shm-transport/src/backend/ring.rs:1417-1422` returns `Ok(None)` on
  `active >= self.grant.max_leases` **before** reading `consumed` or `published`, so
  "every lease is out" is decided without reference to whether anything is queued.
  Frames queued behind it are counted separately, in `SLOT_PUBLISHED`
  (`ring.rs:1696-1705`).
- The counter that reaches the cap: incremented at `ring.rs:1454`
  (`active_leases.fetch_add(1, Relaxed)`) as part of the same block that stores
  `SLOT_RECEIVER_LEASED` and advances `consumed` (`:1452-1454`); decremented only at
  `ring.rs:1592` (`fetch_sub(1, Relaxed)`) inside `release`, after the
  `SLOT_RECEIVER_LEASED → SLOT_RELEASE_PENDING` compare-exchange at `:1575-1580`.
- The drain half reaches the ring through `crates/shm-transport/src/lease.rs:350-357`
  `release_once`, from either the explicit `release()` (`:324-325`) or `Drop`
  (`:368-369`). Either path decrements once, guarded by the local `released` flag.
  New with the eventfd mechanism: the same `release` then signals both the
  `capacity_ready` and `data_ready` doorbells (`ring.rs:1598`), so a consumer
  parked in `wait_for_data` during saturation is woken into the drained state
  rather than observing it on a later poll.
- Observability. Both halves are visible in one `conservation()` snapshot
  (`ring.rs:1607-1624`): `descriptors.receiver_leased` for the held set
  (`:1716-1725`) and `descriptors.published` for the queued backlog (`:1696-1705`).
  The marker therefore needs no new instrumentation, only a snapshot at the moment
  of the `Ok(None)`.
- Where it is reachable today: only `lease_limited_profile()`
  (`crates/shm-transport/tests/ring.rs:18-30`) with `max_leases: 1` and
  `descriptor_depth: 2`, used by
  `lease_limit_reports_backpressure_then_recovers_after_release` (`:214-228`). That
  test does construct saturation and does drain it, so the situation is reached once,
  in one synthetic profile, with a cap of one.
- Where it is not reachable: the shipped host. `max_leases` is `HOST_TEST_RING_DEPTH`,
  which is 8 (`crates/shm-transport/src/profile.rs:679`, `:683-697`), and
  `receive_one` acquires at most one lease per call and releases it on every path —
  the oversized-control rejection
  (`crates/host-runtime/src/ring_transport.rs:685-687`), the normal path (`:734-736`),
  and `Drop` on every error return. One of eight cannot saturate.
- Why a marker rather than a trusted precondition: `max_leases: 1` collapses "at the
  cap" and "one lease held" into the same observation, so a campaign can believe it
  reached saturation while only ever having reached "a lease exists". A cap above one is
  what makes the situation distinct, and nothing today constructs one.
  At HEAD: Release rings only the capacity doorbell; the data doorbell is deliberately left alone, so a consumer that parked on the lease limit is not woken by its own release and must poll again.
  At HEAD: The decrement is a compare-exchange through `Self::advance_cursor`, not a relaxed `fetch_sub`.
  At HEAD: The counter is raised by `Self::advance_cursor`, an `AcqRel` compare-exchange against this handle's own record, not by a relaxed `fetch_add`.
  At HEAD: The gate still runs before `published` is loaded, but `consumed` and `active_leases` now arrive together from `verified_consumer_cursors` at `ring.rs:1416`, ahead of it.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

For a coverage record this section states what it means if the situation never occurs.

If `shm_lease_saturation_observed_then_drained` never fires, then
`receive-resumes-when-lease-capacity-clears` is vacuous: the saturation gate at
`ring.rs:1417` was never taken with a frame pending, so no assertion about resuming
after saturation was ever evaluated. Its check semantics are `always-or-unreached`
precisely because of this, and an unreached verdict is only honest if something
reports the unreachedness. Without the marker the campaign reports a pass, and the
pass means the gate returned `Ok(None)` for the *other* reason — an empty ring —
which is the exact ambiguity that made the existing test's `is_none()` assertion
weak in the first place. Under the eventfd mechanism the unreached state also hides
a wake path: a saturated ring is exactly the state in which `data_available`
(`ring.rs:1501-1512`) parks a `wait_for_data` consumer, so a campaign that never
saturates also never exercises the release-signals-parked-waiter edge
(`:1598`) under lease pressure.
At HEAD: Release signals only the capacity doorbell at HEAD, so there is no release-wakes-a-parked-data-waiter edge to exercise under lease pressure.

A never-fired marker also carries a second, larger message worth reading rather than
suppressing: the shipped host configuration cannot exercise lease backpressure at all.
That is a statement about the deployment, not a defect. It says the eight-lease cap and
the lease-release machinery under it are dead weight in the current topology, and it
tells a reviewer that any confidence in lease backpressure comes from a profile with a
cap of one and a depth of two, not from the profile the host uses.

## Timing windows and dependencies

No race window, because the counter is incremented and decremented by the same
thread-confined receiver and read `Relaxed` at `:1416`. The situation is a state, and
both halves are observable in a single snapshot. The ordering that matters is between
the two halves: the backlog must exist **while** the cap is held, so the snapshot must
be taken at the `Ok(None)`, not before the last acquire and not after the first release.
Taken too early, `descriptors.published` counts frames that will be acquired into the
leased set; taken too late, the cap is already cleared. Dependencies: a profile whose
`max_leases` is small enough to reach and strictly greater than one, and at least one
frame published beyond the leased set, which requires `descriptor_depth > max_leases` so
the extra publication has a slot. `lease_limited_profile()` satisfies the second with
depth 2 and cap 1 but fails the first, so a new profile is required rather than a reuse.
At HEAD: `active_leases` is loaded with `Ordering::Acquire` inside `verified_consumer_cursors` and checked against this handle's private record, not read `Relaxed`.

## What a test must construct

Acquire leases until `active_leases == max_leases` against a profile with a cap of at
least two, with at least one further frame published and unacquired, then observe
`try_receive() == Ok(None)`, then release. Emit the marker
`shm_lease_saturation_observed_then_drained` at the point where a single
`conservation()` snapshot shows `descriptors.receiver_leased == max_leases` **and**
`descriptors.published >= 1`, and where a later snapshot after release shows
`receiver_leased < max_leases`. Both facts in the first snapshot are legal on a correct
system — the comment at `ring.rs:1418-1420` states that a full lease set is
backpressure rather than a fault — and the second snapshot is ordinary progress, so the
marker fires on a correct implementation and never requires a defect. It is not the
negation of any `always` check in this catalog: the violation it pairs with,
"receive never resumes", is a distinct predicate and is not asserted here.

This refines `shm_lease_set_saturated`, already declared in `fault-map.md` as "every
receive lease was held simultaneously". The refinement is deliberate. Saturation alone
does not witness that anything was waiting, and a saturation event with an empty ring
would still leave the recovery property untested. The name is kept distinct so the two
are not conflated, and `shm_lease_set_saturated` should be treated as superseded by it
rather than emitted alongside it.

## Investigation log

### Q: Is lease saturation reachable in any shipped configuration, and is a cap of one sufficient to witness it?

- Sources examined: `ring.rs:1395-1472`, `:1528-1600`, `:1607-1624`; `lease.rs:324-372`;
  `crates/host-runtime/src/ring_transport.rs:32` (source tree; not at HEAD), `:43-55`, `:664-746`;
  `tests/ring.rs:18-30`,
  `:214-228`.
- Findings: not reachable in the shipped host, for a structural reason rather than a
  tuning one — `receive_one` is written to hold exactly one lease for the duration of one
  call. A cap of one is not sufficient to witness the situation, because at that cap
  "saturated" and "one lease held" are the same observation, and the interesting
  behaviour of the gate at `ring.rs:1417` is a comparison against a bound greater than one. Both
  halves of the situation are observable through `conservation()` without new
  instrumentation, which makes the marker cheap.
- Missing evidence: whether the addon receive path can saturate. It retains leases
  deliberately — `poll` forgets the lease and completes through the addon's own identity
  table, per the reachability analysis in
  `release-authority-bound-to-lease-ownership` — so it is the one shipped consumer that
  plausibly reaches the cap. That path was not traced end to end and is recorded as an
  open question rather than claimed.
- Conclusion: resolved with answer — the situation is reachable only synthetically
  today, a cap above one is required for the marker to mean anything, and the marker's
  most valuable outcome may be never firing, because that reports a shipped topology in
  which lease backpressure is unexercised.

### 2026-08-31: re-derivation against the eventfd doorbell mechanism

- Sources examined: `crates/shm-transport/src/backend/ring.rs:1411-1426`,
  `:1452-1454`, `:1501-1512`, `:1575-1598`, `:1607-1624`;
  `crates/shm-transport/src/lease.rs:324-369`;
  `crates/host-runtime/src/ring_transport.rs:664-746`;
  `crates/shm-transport/src/profile.rs:679-697`;
  `crates/shm-transport/tests/ring.rs:18-30`, `:214-228`.
- Findings: the situation, both observable halves, the `sometimes` semantics, and
  the shipped-host unreachability finding all survive PR #131; only line numbers
  moved. The former `DESCRIPTOR_DEPTH` constant in `ring_transport.rs` is gone;
  the depth-and-lease bound now comes from `HOST_TEST_RING_DEPTH` in
  `profile.rs:679`. The drain half gained a mechanism note: the pre-eventfd
  argument was that the caller polls again after the release; at HEAD the release
  signals both doorbells, so a parked `wait_for_data` waiter is woken sparsely,
  and a saturated ring is precisely the state that parks such a waiter. That
  does not change this record's marker, which is state-based, but it is the
  reason the paired recovery record now carries a separate wake arm.
- Missing evidence: unchanged — the addon lease-retention path remains untraced.
- Conclusion: resolved with answer — the record survives with citations
  re-anchored; no semantic change to the marker.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 23, `crates/shm-transport/src/backend/ring.rs:1063-1068` now `crates/shm-transport/src/backend/ring.rs:1417-1422`: The gate still runs before `published` is loaded, but `consumed` and `active_leases` now arrive together from `verified_consumer_cursors` at `ring.rs:1416`, ahead of it.
  - line 28, `ring.rs:1117` now `ring.rs:1454`: The counter is raised by `Self::advance_cursor`, an `AcqRel` compare-exchange against this handle's own record, not by a relaxed `fetch_add`.
  - line 31, `ring.rs:1234` now `ring.rs:1592`: The decrement is a compare-exchange through `Self::advance_cursor`, not a relaxed `fetch_sub`.
  - line 37, `ring.rs:1236-1241` now `ring.rs:1598`: Release rings only the capacity doorbell; the data doorbell is deliberately left alone, so a consumer that parked on the lease limit is not woken by its own release and must poll again.
  - line 78, `:1236-1241` now `:1598`: Release signals only the capacity doorbell at HEAD, so there is no release-wakes-a-parked-data-waiter edge to exercise under lease pressure.
  - line 90, `:1062` now `:1416`: `active_leases` is loaded with `Ordering::Acquire` inside `verified_consumer_cursors` and checked against this handle's private record, not read `Relaxed`.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 128, `crates/host-runtime/src/ring_transport.rs:32` (the DESCRIPTOR_DEPTH constant): The constant is gone; the depth and lease bound is `HOST_TEST_RING_DEPTH` at `crates/shm-transport/src/profile.rs:679`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
