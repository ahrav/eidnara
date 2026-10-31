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
  `exact_len + HEADER_LEN` on the egress budget at [`:517-554`][reserve-direct]
  and returns an [`OutputBuffer`][outbuf] with `direct: Some(DirectOutput)`
  and `body: Vec::new()`. [`output_from_writer`][from-writer] is its only
  entry.
- [`into_parts`][into-parts] returns `OutputParts::Direct(direct, charge)`
  without shrinking; the owned arm shrinks the charge to the allocation.
- [`emit_reserved_frame`][emit-reserved] builds an `EnvelopeHeader` with
  `len: body.len` and a [`DirectFrame`][direct-frame] whose header is that
  header encoded, and queues an `OutboundFrame` with the charge through
  `send_before`.
- On the endpoint thread, [`Publisher::try_publish`][publish-one] reserves
  through `try_reserve_in`, wraps the publish in `catch_unwind`, returns
  `Err(())` unless the result is `Ok(Ok(()))`, and drops `charge` at
  [`:1333-1389`][publish-one] only after success.
- [`publish_direct`][publish-direct] takes the reservation `try_publish`
  made, runs the serializer into a
  [`ReservationWriter`][res-writer]
  under `redact_sync`, then [`commit_before`][commit-before], which refuses at
  the deadline and otherwise calls `commit(body_len)`.
- [`ReservationWriter::write`][res-writer] forwards to
  [`ProducerReservation::write`][res-write], which aborts the reservation and
  marks it finished on any [`write_reservation`][write-res] error; an end past
  `allocation_len` is `Overflow`.
- [`commit`][commit-underfill] aborts with `Underfill` when
  `cursor != body_len` ([`:2533-2570`][commit-underfill]), aborts on
  quarantine or a
  length past capacity, and runs [`prepare_commit`][prepare-commit], which
  checks the wire header's declared length against `body_len` at
  [`:2306-2343`][prepare-commit] before any shared-state write.
- A serializer `Err` returns from `publish_direct` before `commit_before`; a
  panic unwinds through it to `catch_unwind`. In both cases the
  `ProducerReservation` drops, and [`Drop`][res-drop] runs
  [`abort_reservation`][abort], which punches the dirtied range.
- The endpoint loop at [`:696-704`][publish-fail] turns any `Publisher::pump`
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
inbound receives blocked, between `try_reserve_in` returning and
`commit_before`. A slow serializer meets the deadline check at commit. Between
`emit_reserved_frame` queueing the frame and `Publisher::try_publish` completing, the
closure and its captures live in the queue; a retired or cancelled generation
drops the `OutboundFrame` there, which must release both the egress charge
and the captured source bytes. The egress charge's release is at
[`:1333-1389`][publish-one] on success and at the `OutboundFrame` drop otherwise.

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

- Sources examined: [`Publisher::try_publish`][publish-one], the loop at
  [`:696-704`][publish-fail], [`settle_prepared_with`][settle-with], the host
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
[publish-fail]: ../../../../../crates/host-runtime/src/ring_transport.rs#L696-L704
[publish-one]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1333-L1389
[publish-direct]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1392-L1404
[commit-before]: ../../../../../crates/host-runtime/src/ring_transport.rs#L814-L825
[res-writer]: ../../../../../crates/host-runtime/src/ring_transport.rs#L827-L843
[t-deadline]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1849-L1879
[fixture-arm]: ../../../../../crates/host-runtime/tests/support/mod.rs#L441-L455
[abort]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2268-L2304
[prepare-commit]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2306-L2343
[write-res]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2388-L2419
[res-write]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2519-L2531
[commit-underfill]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2533-L2570
[res-drop]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2587-L2594
[settle-with]: ../../../../../crates/daemon/src/lib.rs#L12139-L12194
[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources
[wire63]: ../../../../host-wire-protocol.md#L314
[hr-terminal]: ../../../host-runtime/catalog.md#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame
[hr-pubfail]: ../../../host-runtime/catalog.md#req-a-a-response-publication-failure-never-reaches-the-settling-path

## Dated note (2026-10-14, #1040)

Every line anchor above is as of the recorded head of the investigation. At
the #1040 head the direct path has a production sender: the daemon's
`settle_prepared_via` (`crates/daemon/src/lib.rs:13457`) routes every
transform and exact-length response through `settle_direct_with` (`:13424`)
and `RequestCtx::output_from_direct_write`
(`crates/host-runtime/src/handler.rs:565`) to `reserve_direct`
(`crates/host-runtime/src/dispatch.rs:574`); JSON and emitted bodies keep the
owned path through `settle_prepared_with` (`lib.rs:13351`). The record is
`default-production` and `Exercised: yes`.

Both investigation questions are resolved. A serializer failure on the direct
path closes the connection: the daemon measures every direct body before
reserving it and `write_measured` writes the same immutable source at that
length, so a publication-time mismatch is a writer-integrity fault, while a
measurement failure settles the request as `encode_failed` before any
reservation. Two charges cover a direct frame until commit: the egress charge
of the measured length plus header covers the ring bytes, and the decode's
scratch charges travel with the response through `PreparedOutcome::holding`
(`crates/daemon/src/dispatch.rs:415`), owned by the serializer closure until
it runs or the queued frame drops. A body failure increments
`direct_serialize_failure.observed` in `host.status`
(`crates/host-runtime/src/ring_transport.rs:444-445`); a refused commit does
not.

The fault arms and the charge are tested:
`a_direct_frame_holds_body_plus_header_until_commit_or_returns_it_on_failure`
(`ring_transport.rs:3709`) holds the charge at body plus header while the
frame waits and serializes and returns it after an exact, underfilled,
overfilled, failed, or panicking serializer;
`a_failed_direct_serializer_publishes_nothing_and_returns_its_charge`
(`crates/host-runtime/tests/direct_frames.rs:28`) drives the four
`direct_fill` fault arms (`tests/support/mod.rs:509`) through a real host;
`transform_and_exact_bodies_settle_direct_with_the_owned_bytes`
(`lib.rs:18890`),
`a_held_response_keeps_its_request_charges_until_its_direct_serializer_runs`
(`:18925`), and
`direct_settlement_reserves_the_measured_length_and_stops_when_cancelled`
(`:18964`) cover the daemon branch and its charges;
`a_settled_direct_response_holds_its_decode_charges_until_it_publishes`
(`crates/daemon/src/transform_unit/host_tests.rs:412`) holds a real
transform response between `handle` and publication. The remaining untested
arm is a queued direct frame dropped by generation retirement.
