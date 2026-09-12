# direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit counts one owned `Vec` copy per response and proposes serializing
transform responses directly into the ring through the existing direct
path. That path defers serialization from the handler to the endpoint thread,
which moves the exact-length check, the failure classification, and the
lifetime of the source bytes and their charges to a place no production
sender exercises today.

## Evidence trail

- [`reserve_direct`][reserve-direct] charges exactly
  `exact_len + HEADER_LEN` on the egress budget at [`:528-538`][reserve-direct]
  and returns an [`OutputBuffer`][outbuf] with `direct: Some(DirectOutput)`
  and `body: Vec::new()`. [`output_from_writer`][from-writer] is its only
  entry.
- [`into_parts`][into-parts] returns `OutputParts::Direct(direct, charge)`
  without shrinking; the owned arm shrinks the charge to the allocation.
- [`emit_reserved_frame`][emit-reserved] builds an `EnvelopeHeader` with
  `len: body.len` and a [`DirectFrame`][direct-frame] whose header is that
  header encoded, and queues an `OutboundFrame` with the charge through
  `send_before`.
- On the endpoint thread, [`publish_one`][publish-one] wraps the publish in
  `catch_unwind`, returns `Err(())` unless the result is `Ok(Ok(()))`, and
  drops `charge` at [`:784`][publish-one] only after success.
- [`publish_direct`][publish-direct] calls
  `reserve_until(body_len, header, deadline)`, runs the serializer into a
  [`ReservationWriter`][res-writer]
  under `redact_sync`, then [`commit_before`][commit-before], which refuses at
  the deadline and otherwise calls `commit(body_len)`.
- [`ReservationWriter::write`][res-writer] forwards to
  [`ProducerReservation::write`][res-write], which aborts the reservation and
  marks it finished on any [`write_reservation`][write-res] error; an end past
  `allocation_len` is `Overflow`.
- [`commit`][commit-underfill] aborts with `Underfill` when
  `cursor != body_len` ([`:2551-2555`][commit-underfill]), aborts on
  quarantine or a
  length past capacity, and runs [`prepare_commit`][prepare-commit], which
  checks the wire header's declared length against `body_len` at
  [`:2316-2317`][prepare-commit] before any shared-state write.
- A serializer `Err` returns from `publish_direct` before `commit_before`; a
  panic unwinds through it to `catch_unwind`. In both cases the
  `ProducerReservation` drops, and [`Drop`][res-drop] runs
  [`abort_reservation`][abort], which punches the dirtied range.
- The endpoint loop at [`:622-630`][publish-fail] turns any `publish_one`
  error into `ReadClose::Corrupt("shared-memory publish failed")` and returns,
  which closes the connection. The owned path classifies a `measure` or
  `write_to` failure as a request-scoped `encode_failed` terminal in
  [`settle_prepared_with`][settle-with] before any frame is queued.
- The only production-tree caller of `output_from_writer` is the
  [`direct_fill` fixture arm][fixture-arm]; a repository search finds no sender
  of that kind. [`a_commit_past_the_write_deadline_is_refused`][t-deadline]
  calls `publish_direct` with a 60 ms serializer and a 20 ms deadline and
  asserts no frame is received. The `into_parts` tests at
  [handler.rs:596-673][t-parts] cover the owned arm only.
- [§6.3][wire63] requires writers to verify header `len` equals body length,
  fill the reservation, and publish exactly once, and states that a failed or
  underfilled reservation aborts without publication. The parent's [E2][e2]
  requires each charge to cover its resource's lifetime; the host catalog's
  [publication-failure record][hr-pubfail] covers the owned frame's failure
  arm.

## Failure scenario

A serializer writes `body_len - 1` bytes: `commit` refuses with `Underfill`,
the reservation aborts, the connection closes, and every in-flight request on
it fails. A serializer writes `body_len + 1`: `Overflow` at the writer, same
outcome. A serializer that panics is caught, same outcome. None of these
publishes a frame with a mismatched length, and none leaves a slot or dirty
pages behind. The charge clause fails differently: a design that releases the
request's scratch charge when `handle` returns, while the closure still owns
the source tree until the endpoint thread serializes, undercounts retained
bytes for the whole queue wait.

## Timing windows and dependencies

The serializer runs on the endpoint thread with a ring reservation held and
inbound receives blocked, between `reserve_until` returning and
`commit_before`. A slow serializer meets the deadline check at commit. Between
`emit_reserved_frame` queueing the frame and `publish_one` completing, the
closure and its captures live in the queue; a retired or cancelled generation
drops the `OutboundFrame` there, which must release both the egress charge
and the captured source bytes. The egress charge's release is at
[`:784`][publish-one] on success and at the `OutboundFrame` drop otherwise.

## What a test must construct

Direct frames whose serializer writes `body_len - 1`, `body_len + 1`,
returns `Err` after some bytes, or panics, each asserting no received frame,
`resident_arena_pages` unchanged from before the reservation, and the next
reservation succeeding; a body that wraps the arena end; a deadline expiring
during serialization; and, with the egress budget near capacity, an
observation that the charge equals `exact_len + HEADER_LEN` until commit and
that a queued frame dropped by generation retirement returns it. The
[ring checks](../existing-checks.md#ring-arena-and-direct-frame) cover the
deadline arm and the owned `into_parts` cases only.

## Investigation log

### Q: Is a connection close the intended outcome for a settled response?

- Sources examined: [`publish_one`][publish-one], the loop at
  [`:622-630`][publish-fail], [`settle_prepared_with`][settle-with], the host
  catalog's [terminal record][hr-terminal], and [§6.3][wire63].
- Findings: The owned path fails one request; the direct path fails the
  connection. §6.3 fixes abort-without-publication and says nothing about
  which scope the failure is reported to. The
  terminal record holds trivially on the failure arm because nothing is
  emitted, and the settled `Response` is then never delivered.
- Missing evidence: A decision on which classification the direct path must
  preserve for transform responses.
- Conclusion: needs human input.

### Q: Which charge class covers the captured source bytes?

- Sources examined: [`reserve_direct`][reserve-direct],
  [`into_parts`][into-parts], the [`RequestCtx::scratch`][scratch] doc,
  [E2][e2].
- Findings: The egress charge covers `exact_len + HEADER_LEN` of ring bytes,
  not the source tree. The scratch budget is documented for request-derived
  ownership whose lifetime is the request. A closure that outlives `handle`
  extends the source tree's lifetime past both definitions.
- Missing evidence: A design decision; the path has no production sender.
- Conclusion: needs human input.

### Q: Does the terminal record already cover the undelivered response?

- Sources examined: [hr-terminal], [hr-pubfail].
- Findings: The publication-failure record covers owned frames. The direct
  arm reaches the same `fail` call, so the same record applies once the
  direct path carries responses; this evidence does not extend it.
- Missing evidence: None; this is a relationship note.
- Conclusion: resolved with answer - the host catalog's publication-failure
  record is the owner, and this record adds the direct arm's charge clause
  only.

[reserve-direct]: ../../../../../crates/host-runtime/src/dispatch.rs#L517-L554
[emit-reserved]: ../../../../../crates/host-runtime/src/dispatch.rs#L282-L332
[outbuf]: ../../../../../crates/host-runtime/src/handler.rs#L320-L335
[into-parts]: ../../../../../crates/host-runtime/src/handler.rs#L387-L400
[scratch]: ../../../../../crates/host-runtime/src/handler.rs#L441-L444
[from-writer]: ../../../../../crates/host-runtime/src/handler.rs#L465-L472
[t-parts]: ../../../../../crates/host-runtime/src/handler.rs#L596-L673
[direct-frame]: ../../../../../crates/host-runtime/src/frame_channel.rs#L166-L200
[publish-fail]: ../../../../../crates/host-runtime/src/ring_transport.rs#L622-L646
[publish-one]: ../../../../../crates/host-runtime/src/ring_transport.rs#L749-L786
[publish-direct]: ../../../../../crates/host-runtime/src/ring_transport.rs#L788-L800
[commit-before]: ../../../../../crates/host-runtime/src/ring_transport.rs#L814-L825
[res-writer]: ../../../../../crates/host-runtime/src/ring_transport.rs#L827-L843
[t-deadline]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1857-L1887
[fixture-arm]: ../../../../../crates/host-runtime/tests/support/mod.rs#L441-L455
[abort]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2268-L2304
[prepare-commit]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2306-L2343
[write-res]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2388-L2419
[res-write]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2519-L2531
[commit-underfill]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2533-L2570
[res-drop]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2587-L2594
[settle-with]: ../../../../../crates/daemon/src/lib.rs#L12020-L12076
[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources
[wire63]: ../../../../host-wire-protocol.md#L314
[hr-terminal]: ../../../host-runtime/catalog.md#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame
[hr-pubfail]: ../../../host-runtime/catalog.md#req-a-a-response-publication-failure-never-reaches-the-settling-path
