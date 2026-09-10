# arena-payload-copies-keep-the-address-derived-atomic-shape

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit counts one zero-fill plus one copy per inbound frame in `to_vec` and
one atomic-wise copy per outbound frame in `copy_in`, and proposes a
`memcpy` or an uninitialized destination. Both proposals touch the crate's
verification boundary: the byte-width discipline that turns a concurrent peer
store into stale data instead of a data race, and the initialization proof
that keeps uninitialized process memory out of a payload.

## Evidence trail

- [`LeaseSpan`][span-doc] has no `&[u8]` accessor; its `new` contract at
  [`:25-33`][span-safety] requires that no `&[u8]` or `&mut [u8]` is formed
  over the bytes and that a concurrent writer use the span's exact base and
  length, because a shifted range assigns another width to shared bytes.
  [`as_mut_ptr`][span-ptr] is documented for exclusive pre-commit writes only.
- [`AccessShape::of(address, len)`][shape] derives `head`, `words`, and
  `tail` from the absolute address and length alone, so two parties on the
  same range agree on every byte's width.
- [`copy_out`][copy-out] and [`copy_in`][copy-in] partition the range by the
  shape and use `AtomicU8`/`AtomicU64` relaxed loads and stores; each carries
  a `SAFETY` comment that the shape partitions exactly the range, every word
  start is 8-aligned and in bounds, and the caller keeps the range valid with
  no Rust reference over it. [`read_byte`][read-byte] and [`checksum`][checksum]
  use the same shape; [`copy_to`][copy-to] refuses a destination whose length
  differs from the span's.
- [`to_vec`][to-vec] allocates `vec![0u8; self.body_len]` at
  [`:331`][to-vec-fill], then for each span takes `bytes.get_mut(cursor..end)`
  and calls `copy_to`; a span that overruns returns `LengthMismatch`, and a sum
  short of `body_len` returns `LengthMismatch` at [`:344-346`][to-vec]. Every
  early `Err` leaves the `Vec` unobservable; the zero-fill makes every byte
  initialized before the first copy.
- The ring forms spans through [`LeaseSpan::new`][ring-span] with a `SAFETY`
  comment that no `&[u8]` over the arena is ever formed in the crate, and
  writes reservations through [`write_reservation`][write-res], whose
  `SAFETY` comment names `copy_in` and exclusive ownership until commit.
- On the host, [`receive_one`][receive-to-vec] calls `to_vec` on every inbound
  frame; `publish_owned` and `ReservationWriter` write through
  [`ProducerReservation::write`][res-write] to `write_reservation` on every
  outbound frame.
- [`AGENTS.md`][agents] names unsafe code in the crate a verification
  boundary and requires the Miri and Valgrind commands from `ci.yml`. The
  [Miri job][ci-miri] runs `lease::` and `backend::ring::miri` tests; the
  [Valgrind job][ci-valgrind] runs `shm-transport --test ring` under memcheck.
- Tests:
  [`copy_in_then_copy_out_round_trips_at_every_alignment_and_length`][t-roundtrip]
  (16 offsets x 16 shifts x 40 lengths),
  [`read_byte_agrees_with_copy_to_at_every_alignment`][t-readbyte], and
  [`span_reads_tolerate_a_concurrent_writer`][t-concurrent], which spawns a
  same-shape `copy_in` writer against `read_byte`, `copy_to`, and `checksum`,
  with 4 rounds under Miri and 64 natively.
- The shm-transport catalog's [no-reference record][shm-noref] owns the
  general prohibition; this record adds the copy shape and the `to_vec`
  initialization clause.

## Failure scenario

A `memcpy` replacement forms a `&[u8]` or `&mut [u8]` over peer-writable
memory, a data race the peer can trigger at will. A copy that uses another
partition than `AccessShape::of` makes the host and peer disagree on a byte's
width, a mixed-size data race that Miri flags and the hardware may tear. A
`to_vec` that allocates with capacity and `set_len` or `MaybeUninit`, then
returns `Err` after a partial copy or after a span-length mismatch, either
leaks the partially written buffer into the handler or must prove per span
that every byte below `cursor` was written before the `Vec` is observable.

## Timing windows and dependencies

The peer may store into a leased span at any time; the property is about the
shape of concurrent accesses, not their order. The `to_vec` clause is
sequential: every span-length check runs per span, so an initialization proof
without the zero-fill depends on those checks, and on `copy_to`'s exact-length
refusal, running before any byte is exposed.

## What a test must construct

Under Miri: a `to_vec` over a lease whose span lengths do not sum to
`body_len`, both short and long, asserting `Err(LengthMismatch)` and no
uninitialized read; a span starting at each of the eight word offsets; the
existing concurrent-writer thread test. Natively and under Valgrind: the ring
integration tests. Any replacement copy must be added to the sets the
[Miri job][ci-miri] filters on. The
[ring checks](../existing-checks.md#ring-arena-and-direct-frame) hold the
three lease tests and both CI jobs; none covers mismatched span lengths.

## Investigation log

The catalog record lists no open questions. One question was checked while
tracing the initialization clause.

### Q: Does any `to_vec` error path expose a partially written buffer?

- Sources examined: [`to_vec`][to-vec], [`copy_to`][copy-to],
  [`copy_out`][copy-out].
- Findings: `to_vec` owns `bytes` locally and returns it only on `Ok`. The
  zero-fill at [`:331`][to-vec-fill] initializes every byte before any copy;
  `get_mut` bounds each span and `copy_to` refuses a length mismatch, so no
  path returns `Ok` with a byte `copy_out` did not write.
- Missing evidence: None at HEAD; the clause is about the replacement.
- Conclusion: resolved with answer - no path at HEAD exposes uninitialized or
  partial bytes; a zero-fill-free replacement must re-establish that per span.

[agents]: ../../../../../crates/shm-transport/AGENTS.md
[span-doc]: ../../../../../crates/shm-transport/src/lease.rs#L10-L13
[span-safety]: ../../../../../crates/shm-transport/src/lease.rs#L25-L33
[span-ptr]: ../../../../../crates/shm-transport/src/lease.rs#L49-L52
[read-byte]: ../../../../../crates/shm-transport/src/lease.rs#L62-L82
[copy-to]: ../../../../../crates/shm-transport/src/lease.rs#L84-L93
[checksum]: ../../../../../crates/shm-transport/src/lease.rs#L95-L123
[shape]: ../../../../../crates/shm-transport/src/lease.rs#L134-L156
[copy-out]: ../../../../../crates/shm-transport/src/lease.rs#L176-L206
[copy-in]: ../../../../../crates/shm-transport/src/lease.rs#L208-L235
[to-vec]: ../../../../../crates/shm-transport/src/lease.rs#L328-L348
[to-vec-fill]: ../../../../../crates/shm-transport/src/lease.rs#L331
[t-roundtrip]: ../../../../../crates/shm-transport/src/lease.rs#L475-L506
[t-readbyte]: ../../../../../crates/shm-transport/src/lease.rs#L543
[t-concurrent]: ../../../../../crates/shm-transport/src/lease.rs#L581-L619
[ring-span]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2060-L2068
[write-res]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2388-L2419
[res-write]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2519-L2531
[receive-to-vec]: ../../../../../crates/host-runtime/src/ring_transport.rs#L731-L736
[ci-miri]: ../../../../../.github/workflows/ci.yml#L597-L635
[ci-valgrind]: ../../../../../.github/workflows/ci.yml#L637-L669
[shm-noref]: ../../../shm-transport/catalog.md#no-rust-reference-over-peer-writable-payload
