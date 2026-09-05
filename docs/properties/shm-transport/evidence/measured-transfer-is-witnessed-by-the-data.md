# measured-transfer-is-witnessed-by-the-data

## Discovery trigger

Every `Measurement` emitted by the hardware envelope bench carries a `checksum`
field (`benches/hardware_envelope.rs:74`), and the report presents arms
side by side. A checksum in a transfer benchmark is normally the one field that
proves the transfer happened, so each arm's checksum was traced to the value it
is computed from.

## Evidence trail

The ring family — `h1_raw_descriptor_ring_payload_touch`,
`direct_producer_leased_receiver`, `ring`, and the three copied ablations, all
dispatched to `run_ring` at `benches/hardware_envelope.rs:317-332` — computes
its checksum at `:675-677`:
At HEAD: That closed form is now the expected value, not the reported one: `run_ring` loads the consumer's accumulated checksum at `:678` and fails the arm with "ring peer observed a different payload than the producer published" when the two disagree (`:679-681`), so the reported checksum is witnessed by delivered bytes.
At HEAD: `h1_raw_descriptor_ring_payload_touch` no longer dispatches to `run_ring`; it returns an error (`:319-321`), so the ring family is `direct_producer_leased_receiver`, `ring`, `injected_avoidable_operations`, and the three copied ablations.

```rust
let checksum = iterations
    .wrapping_mul(payload_len as u64)
    .wrapping_mul(0x5a);
```

The inputs are the two loop parameters and the fill byte. No delivered byte
participates. `:331` (source tree; not at HEAD) then passes the value through `black_box`, which prevents
the compiler from folding it but does not make it an observation.

The consumer does compute a real checksum. `ring_consumer` at `:764-769`
iterates the lease segments and evaluates `black_box(span.checksum())` at
`:768`. The result is discarded on the spot; it is never compared, accumulated,
or returned. On the copied-receiver path the bytes are copied by
`lease.to_vec()` at `:757` and only `is_err()` is inspected, so the copied
bytes are never examined either.
At HEAD: The copied bytes are examined now: they are folded into the same running checksum at `:760-762`.
At HEAD: The value is no longer discarded or wrapped in `black_box`: it accumulates into `checksum` and is published to the producer through `PeerReport` at `:777-786`.

The other arm families do checksum real data:

- `run_stream_pair` (`unix_socket`, `tcp`) and `run_iceoryx` accumulated over
  bytes actually read back (pre-#131 `:516` and `:587-594`); both arms were
  removed by the #131 bench rewrite, so no payload-witnessing comparison arm
  remains in the file.
- `run_h0` reports `(*line).load(Ordering::Relaxed)` at `:621`, which is the
  final ping-pong counter, not a checksum over any payload. `h0` transfers no
  payload, so there is nothing for it to witness.

The consequence is that the `checksum` column mixes three unrelated quantities:
a closed form over parameters (ring family), a byte sum (stream and iceoryx
families), and a protocol counter (h0). Within the ring family the value is a
constant for a given `(iterations, payload_bytes)` pair, so all six ring-family
arms report the identical checksum regardless of whether the receiver leased or
copied.

One partial oracle does exist and should not be overstated away: total delivery
failure is caught by the child exit status rather than the checksum. If
`try_receive` never yields, `ring_consumer` returns `2` at `:753`,
`wait_child` reports it at `:518`, and `run_ring` returns
`Err("ring peer failed")` at `:673`. So a frame that is never delivered fails
the arm. A frame that is delivered with wrong bytes does not.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A defect corrupts payload bytes between `reservation.write(source)` (`:718`)
and the consumer's view of the segment. The consumer computes
`span.checksum()` at `:768` over the corrupted bytes and throws the result
away. `lease.release()` succeeds, the child exits `0`, and the parent computes
`iterations * payload_len * 0x5a` at `:675-677`. The emitted record is
bit-identical to a correct run: same `elapsed_ns` distribution, same counters,
same checksum. The `ring` arm is among the emitted `paired_process_arms`
(`:289`), so this is the
arm a shipping decision would rest on.
At HEAD: The fill byte is now the `BODY_BYTE` constant and the product is only the expectation; a mutated byte would make the consumer's reported checksum differ and the arm would fail at `:679-681` instead of emitting a bit-identical record.

## Timing windows and dependencies

None required to reach the gap; it is present in every invocation. A corruption
window is required only to demonstrate the impact, and the corruption need not
be timed: any deterministic mutation of the delivered bytes reproduces the
identical checksum.

The one dependency worth naming is that the corruption must not stall delivery.
If it also prevents `try_receive` from yielding, the exit-status oracle at
`:672-674` catches it and the checksum gap is masked.

## What a test must construct

1. A run of the `ring` arm with one delivered byte mutated after commit and
   before the consumer reads it, asserting that the reported `checksum`
   differs from the unmutated run. This fails today by construction.
2. A cross-arm comparability assertion: for a fixed `(iterations, payload)`,
   assert the ring-family checksum is not equal to the value produced by the
   same payload through `run_stream_pair`, or else assert that both are defined
   over the same quantity. Either outcome is informative; the current state is
   that the field is not comparable and nothing says so.
3. A wiring change is implied for both: the consumer's `span.checksum()` at
   `:768` must be accumulated and returned across the fork boundary, since the
   process that sees the bytes is the child and the process that emits the
   record is the parent.
   At HEAD: The wiring change this asked for exists: the checksum crosses the fork boundary through the shared `PeerReport` (`:118`, published at `:777-786`) and the parent compares it at `:678-681`.

## Investigation log

The catalog records no open question for this property. The question actually
resolved during the trail is logged so the reasoning is reproducible.

### Q: Does any arm's reported checksum depend on delivered bytes, and is the ring arm's value therefore comparable to the stream arms'?

- Sources examined: `benches/hardware_envelope.rs` in full, specifically
  `run_h0` (`:548-623`), `run_ring` (`:630-697`), `ring_consumer`
  (`:736-788`), the removed `run_stream_pair` and `run_iceoryx` arms
  (pre-#131 `:487-529` and `:531-598`; deleted by the bench rewrite),
  and the `Measurement` struct (`:57-76`).
- Findings: the stream arms and the iceoryx arm computed their checksum from
  received bytes before their removal. The ring family does not; its value is a pure function of
  `iterations`, `payload_len`, and the literal `0x5a`. `h0` reports a protocol
  counter. The consumer's genuine `span.checksum()` is computed at `:768` and
  discarded.
- Missing evidence: nothing. Every arm's checksum expression was read directly
  at `9c1eb4d1` and no arm dispatches through a path not listed above.
- Conclusion: resolved. No ring-family arm's checksum is witnessed by delivered
  data, and the field is not comparable across arm families. Total
  non-delivery is still caught, by the child exit status rather than by the
  checksum.
  At HEAD: It is no longer discarded: the ring family's reported checksum is exactly this accumulated value, checked against the closed form before the arm is allowed to succeed.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 15, `benches/hardware_envelope.rs:114-124` now `benches/hardware_envelope.rs:317-332`: `h1_raw_descriptor_ring_payload_touch` no longer dispatches to `run_ring`; it returns an error (`:319-321`), so the ring family is `direct_producer_leased_receiver`, `ring`, `injected_avoidable_operations`, and the three copied ablations.
  - line 16, `:328-330` now `:675-677`: That closed form is now the expected value, not the reported one: `run_ring` loads the consumer's accumulated checksum at `:678` and fails the arm with "ring peer observed a different payload than the producer published" when the two disagree (`:679-681`), so the reported checksum is witnessed by delivered bytes.
  - line 30, `:371` now `:768`: The value is no longer discarded or wrapped in `black_box`: it accumulates into `checksum` and is published to the producer through `PeerReport` at `:777-786`.
  - line 32, `:363` now `:757`: The copied bytes are examined now: they are folded into the same running checksum at `:760-762`.
  - line 65, `:328-330` now `:675-677`: The fill byte is now the `BODY_BYTE` constant and the product is only the expectation; a mutated byte would make the consumer's reported checksum differ and the arm would fail at `:679-681` instead of emitting a bit-identical record.
  - line 93, `:371` now `:768`: The wiring change this asked for exists: the checksum crosses the fork boundary through the shared `PeerReport` (`:118`, published at `:777-786`) and the parent compares it at `:678-681`.
  - line 112, `:371` now `:768`: It is no longer discarded: the ring family's reported checksum is exactly this accumulated value, checked against the closed form before the arm is allowed to succeed.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 25, `:331` (black_box around the reported checksum): No `black_box` wraps the checksum at HEAD; the only remaining `black_box` in the ring path is over the source body in `produce` (`:732`).
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
