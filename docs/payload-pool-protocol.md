# Payload Pool Protocol

This document defines how the shared-memory channel carries a complete frame
between two processes. It covers mapping layout, descriptors, completion cells,
wakes, identifiers, and access to shared memory.

`docs/host-wire-protocol.md` Section 7.7 defines the application protocol:
the 21-byte header, JSON bodies, control operations, send outcomes, and the
exact 67,108,864-byte body maximum.

Wire names and literals here are versioned. Changing any of them requires a
protocol change and a new layout version. There is no negotiation, fallback,
or second layout.

Implementation: `crates/shm-transport/src/{pool.rs, backend/retained.rs,
backend/ring.rs, lease.rs, descriptor.rs, profile.rs}`.

## 1. Identifiers

| Identifier | Value | Where it is checked |
| --- | --- | --- |
| Descriptor schema | `4` | `GrantMessage.descriptor_schema` on the setup socket; `TargetProfile::new`; the native addon's `descriptorSchemaVersion()` |
| Mapping layout version | `4` | `PoolGrant::decode`; the lifecycle page of every mapping |
| Hardware profile | `host-payload-pool-v1` | `WireDescriptor.profile` on the setup socket; `RingClientEndpoint::attach_with_descriptors`; the native addon's `attach` and `begin_connect` |
| Application header version | unchanged (`WIRE_V3_VERSION`) | byte 4 of every frame header |
| Discovery schema | unchanged (`2`) | connection file |

A mismatch in any identifier retires the connection before application
traffic. Peers do not decode old layouts. There is no compatibility matrix.

## 2. Geometry

Each direction has one pool with fixed block classes. Each block holds one
complete frame, header first. Both peers compute its fixed offset from the
same validated `PoolGeometry`. A peer cannot move a block by writing to the
mapping.

| Inventory | Class | Full-frame bytes per block | Blocks |
| --- | --- | ---: | ---: |
| Ordinary | 0 | 4,096 | 64 |
| Ordinary | 1 | 65,536 | 16 |
| Ordinary | 2 | 1,048,576 | 8 |
| Ordinary | 3 | 8,388,608 | 2 |
| Ordinary | 4 | 67,112,960 (64 MiB + 4 KiB) | 1 |
| Control | 5 | 4,096 | 32 |
| Terminal | 6 | 32,768 | 64 |

Body capacity of a class is its block size minus the 21-byte header. Class
boundaries for ordinary bodies are therefore 4,075; 65,515; 1,048,555;
8,388,587 bytes; the largest class accepts exactly the protocol maximum of
67,108,864 body bytes. Block ids are dense in table order: ordinary class 0
holds ids 0-63, class 1 holds 64-79, and so on; control blocks are 91-122 and
terminal blocks are 123-186. There are 187 blocks, 187 completion cells, and
187 return records per direction.

Descriptor slots per direction: 32 ordinary plus 16 reserved, a queue depth
of 48. Ordinary reservations may hold at most 32 unconsumed descriptors;
control and terminal reservations may use all 48.

Ordinary reservations take the smallest ordinary class whose body capacity
covers the caller's bound. There is no spill into a larger class, shared free
list, or coalescing. An empty class refuses the reservation (`Exhausted`), even
when larger blocks are free. Control reservations use only the control class.
Terminal reservations use only the terminal class. A body larger than the
inventory's largest class is refused (`BoundExceedsClass`) before any block is
taken.

Block backing per direction is 95,817,728 bytes (89.254 MiB ordinary plus
2.125 MiB reserved). Every profile, including test profiles, must fit one
maximum frame in its largest ordinary class. The geometry validator caps both
blocks and descriptors at 4,096. A hostile grant therefore cannot force a
large allocation before validation.

## 3. Grant

`PoolGrant` is 126 little-endian bytes sent as lowercase hex inside the setup
descriptor, one per direction:

| Offset | Bytes | Field |
| ---: | ---: | --- |
| 0 | 2 | layout version, `4` |
| 2 | 16 | incarnation, drawn from the OS random source per pool |
| 18 | 4 | lane (`0` host-to-peer, `1` peer-to-host) |
| 22 | 4 | ordinary descriptors |
| 26 | 4 | reserved descriptors |
| 30 | 7 x 12 | seven classes in table order: block bytes (8), count (4) |
| 114 | 8 | total mapping bytes |
| 122 | 4 | reserved, zero |

The decoder validates the geometry and recomputes the mapping layout at the
host page size. It refuses a grant if the total differs from the computed
layout, the reserved tail is nonzero, or the version is not 4.

The setup layer also requires both grants to match the sole profile's geometry.
The host-to-peer field must carry lane `0`; the peer-to-host field must carry
lane `1`. Both pools must have no traffic in flight (`Ring::is_fresh`).

## 4. Mapping layout

Each direction uses one memfd with seals
`F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL`, mode `0600`, and the creating
user as owner. Before mapping, the attaching peer verifies seals, size, type,
mode, and owner. Regions appear in this order, with 128-byte control pages:

| Region | Size | Writers |
| --- | --- | --- |
| Producer page: `published: AtomicU64` | 128 | producer |
| Consumer page: `consumed: AtomicU64` | 128 | consumer |
| Data wake epoch: `generation`, `parked` (`AtomicU64` each) | 128 | producer bumps `generation` and clears `parked`; consumer sets `parked` |
| Capacity wake epoch: `generation`, `parked` | 128 | consumer and any lease owner bump `generation` and clear `parked`; producer sets `parked` |
| Lifecycle page | 256 | creator, once; `quarantined: AtomicU8` by either peer |
| Descriptor slots: 48 x 64 bytes, each `sequence`, `block`, `generation`, `body_len` (`AtomicU64`) | 3,072 | producer |
| Completion cells: 187 x `AtomicU64`, padded to 128 | 1,536 | the final owner of each block's lease |
| Return summary: 3 x `AtomicU64`, one bit per block, cacheline aligned | 24 | the final owner of each block's lease sets a bit; the producer clears whole words |
| Block arena, page aligned | 95,817,728 | producer while reserved; nobody while published |

Total at 4 KiB pages: 95,825,920 bytes per direction. Every byte of every
control structure lies inside an atomic or an `UnsafeCell`, so a shared
reference to a page tolerates concurrent peer stores. Plain lifecycle fields
are read with volatile loads and never referenced.

The lifecycle page carries magic `0x4d43_5348_4d50_3034`, the layout version,
lane, incarnation, total bytes, both descriptor depths, and the seven class
sizes and counts. Attachment compares every field with the grant.

## 5. Publication

The producer keeps a private ledger: one state (`Free`, `Reserved`,
`Published`) and one reuse generation per block, one free-index list per class,
and one list of published blocks. The ledger is allocated before activation and
never grows.

1. **Reclaim.** Before every reservation and on every capacity wake, the
   producer swaps each return-summary word to zero with `Acquire`. It visits
   only blocks whose bits were set. A payload still held by a reader adds no
   work per reservation.

   The producer checks each visited cell against its ledger as a
   `CompletionRecord`. A cell equal to the block's issued generation returns
   a published block to its class free list. A cell behind that generation,
   or a flag for an already reclaimed block, is stale and frees nothing.
   A cell ahead of any issued generation is a protocol error that quarantines
   the pool. So is a flag for a block id outside the geometry.

   A cell written without its flag is still caught. Reservation refuses to
   reuse a free block whose cell is at or past the generation it would issue.
   `Ring::probe` also scans every published block's cell.
2. **Descriptor headroom.** `published - consumed` is read with `Acquire` and
   checked for monotonicity and depth. Ordinary reservations need it below 32;
   reserved reservations need it below 48.
3. **Block.** Pop the smallest fitting class; increment the block's generation.
   A generation that would wrap retires the producer (`Retired`) without
   touching any live lease.
4. **Fill.** The caller writes body bytes through `ProducerReservation::write`
   or an in-place `LeaseSpan` whose length is the caller's bound, never the
   class slack. Writes use relaxed atomic stores of the width `AccessShape`
   assigns to each byte.
5. **Commit.** `commit(body_len)` requires `body_len == written()`. The header
   must declare that `body_len` and carry the application version byte.
   The producer copies the header into the block. It then stores `block`,
   `generation`, `body_len`, and finally `sequence` into slot
   `(sequence - 1) % 48` with relaxed stores. It advances `published` from its
   private record to `sequence` with an `AcqRel` compare-exchange. A sequence
   that would wrap retires the producer before any store.
6. **Wake.** Bump the data wake generation (`SeqCst`) and, if `parked` was
   nonzero, send one byte on the data doorbell. `WouldBlock` means a token is
   already pending. Any other send error quarantines the pool; publication is
   never rolled back.

Abort (`ProducerReservation::abort` or drop) returns an unpublished block to its
free list immediately; the burned generation is never reused. A committed frame
shorter than its bound keeps its block until the payload returns; there is no
relocation after serialization.

## 6. Consumption

1. Read `published` with `Acquire`; reject a value below the greatest one seen
   or more than 48 ahead of `consumed`.
2. Copy the slot's four fields out with relaxed loads. Validate, in order:
   `sequence == consumed + 1`; block id inside the geometry; generation nonzero;
   `body_len <= 67,108,864`; `body_len` within the block's capacity; the
   receiver's own record shows no live lease on the block; the generation
   exceeds the greatest one the receiver has accepted for the block.
3. Copy the 21-byte header out of the block and require its declared length to
   equal `body_len` and its version byte to be the application version.
4. Advance `consumed` from the private record to `sequence` with an `AcqRel`
   compare-exchange, mark the block live in the receiver record, and signal the
   capacity doorbell: consumption alone frees a descriptor slot.
5. Return an owned `PayloadLease`.

Any validation failure quarantines the pool and exposes no byte. Consumers must
not require consecutive generations: an aborted reservation burns one.

## 7. Owned leases and completion

`PayloadLease` holds an `Arc<Retained>` (the mapping, completion cells, receiver
records, and the local capacity doorbell end), the block id, the captured
generation, the body length, and the captured header. It is `Send`; `Ring`,
`ProducerReservation`, and `LeaseSpan` are not. The lease exposes the body only
as a lexical `LeaseSpan` whose reads are relaxed atomics, and as `to_vec`.

The final owner returns the payload exactly once, on `release` or drop, from
any thread:

1. Clear the block's live record (`AcqRel` compare-exchange from the captured
   generation to zero).
2. Publish the captured generation into the block's completion cell with
   `fetch_max(Release)`, then set the block's bit in its return-summary word
   with `fetch_or(Release)`. Publication is monotonic: a stale return can never
   lower a newer completion, and the bit it sets names a cell the producer then
   finds stale.
3. Bump the capacity wake generation (`SeqCst`) and, if `parked` was nonzero,
   send one byte on the retained capacity doorbell end. `WouldBlock` is
   success; any other error is reported to an explicit `release` caller as
   `WakeFailed` and discarded by drop. The return itself stands either way.

The final drop performs no `Ring` call, waits for no slot, and mutates no free
list. Test builds observe those three as `unreachable` code points
(`lease::observers`). It also allocates no completion node and makes no N-API
call. Neither has a code point in this crate. The pool publishes into a fixed
cell. The N-API boundary lives in the native addon; its detach-before-return
rule keeps N-API calls out of a final drop.

The last holder's drop unmaps the backing and settles its admission charge.
An endpoint exit therefore cannot unmap a block a reader still holds.
A return after endpoint exit publishes only into the retired pool's cells.

## 8. Wake channels

Each direction has two connected `AF_UNIX` stream socketpairs: data-ready and
capacity-ready. The creator keeps one end of each and transfers the other in the
grant. Attachment accepts only a connected `SOCK_STREAM` socket (`SO_TYPE` and
`getpeername`); an eventfd, datagram, seqpacket, or unconnected socket is
refused before traffic. Each end is its own open file description, so a peer
clearing `O_NONBLOCK` cannot make the local end block.

Waiters arm by generation, recheck, drain the coalesced token, recheck, then
block. Signalers bump the generation, clear `parked`, and send one token only
when a waiter was parked. A zero-length read means the peer closed its end and
quarantines the local handle; `EPIPE` on send likewise. Doorbell EOF proves the
peer closed its doorbell holders, not that it unmapped the pool.

## 9. Quarantine, retirement, and accounting

The peer that observes impossible shared state sets `quarantined` on the
lifecycle page. Each handle also records quarantine privately. Clearing the
shared flag cannot revive a handle. A quarantined pool refuses reservation,
publication, and receive. Outstanding leases keep reading and return normally.

Retirement (`Retired`) happens when a sequence or generation would wrap. The
producer grants no further reservation; live leases, charges, and the peer's
consumption are unaffected.

Before activation, admission charges each connection for the complete layout
(`mapping_bytes`), private ledgers (`ledger_bytes`), descriptors, blocks
(`leases`), mappings, file descriptors, retained wake handles, and client
instance. The worker charge refunds when the endpoint thread exits.

Both directions share the backing charge. It refunds after the last lease
returns and both handles drop, or moves to the quarantined bucket.
The host treats end-of-file on both data doorbells as proof that the peer
dropped its rings and every lease. If the peer still holds either two seconds
after the host endpoint retires, the backing charge moves to the quarantined
bucket. If quarantine accounting fails, the charge stays active for the
process lifetime. Storage whose release is unproved is never refunded.

One connection commits 191,668,296 bytes: two mappings of 95,825,920 bytes and
two ledgers of 8,228 bytes (187 blocks at 44 bytes). The host uses a fixed
1 GiB ceiling (`MAX_RING_RESIDENT_BYTES` in
`crates/host-runtime/src/ring_transport.rs`). The default and maximum
`max_connections` is therefore 5. The replaced FIFO ring charged 64 MiB per
direction and fitted 8.

Blocks are backed on first touch. Pages are never returned to the kernel, so
resident bytes grow toward the connection's charge over its lifetime.
The ceiling bounds resident bytes, not only virtual commitment. Quarantined
charges count against the same ceiling. Five quarantined connections exhaust
the process until it restarts.

## 10. Source-access contract

The mapping is peer-writable for its whole lifetime. Legal access to shared
bytes in this crate is:

- Fixed-width atomics on control pages, slots, and completion cells, with the
  orderings above.
- `copy_in` and `copy_out`: relaxed atomic stores and loads of the width
  `AccessShape` derives from the absolute address and length, so two parties
  touching the same range agree on every byte's width.
- `LeaseSpan::read_byte`, `copy_to`, and `checksum`, which use the same shape.

No `&[u8]` or `&mut [u8]` over the arena is ever formed. A copy stabilizes the
destination bytes, not the source. A peer that writes a published block
violates the protocol. If the write uses the block's exact span, readers may
observe stale or torn bytes, but not undefined behavior.

That guarantee depends on access shape. Concurrent accesses must be disjoint
or share identical boundaries (`LeaseSpan::new`). Overlapping ranges with
shifted boundaries assign different widths to the same bytes. That mixed-size
race is outside this contract. Consumers therefore decode only private copies.
Before parsing, they validate the copied header against the copied body length.

The producer's raw-pointer escape is `ProducerReservation::segment`, whose span
is exactly the caller's bound; the consumer's is `PayloadLease::body`, whose
span is exactly the declared body. The native addon aliases those spans as
external `ArrayBuffer`s and detaches them before commit and before return.
Concurrent non-atomic JavaScript writes to a span the peer is reading are a
protocol violation the shape does not cover; the addon's detach-before-publish
rule is what excludes them.

## 11. Ordering promises

Within one direction, descriptor consumption is FIFO by publication sequence.
Queue order is not a cross-class publication promise: a producer may publish a
reserved control or terminal frame while an ordinary frame waits for its class
or for ordinary descriptor headroom. Application ordering rules (Request
correlation order, per-stream data before terminal, drain before `Goodbye`)
are the Host Wire Protocol's and are enforced by the publisher's selection
policy, not by this layer.

The host publisher (`Publisher` in `crates/host-runtime/src/ring_transport.rs`)
keeps pending frames in admission order and selects inventory as follows:

- Pure-header `Ping`, `Pong`, `Cancel`, and `Goodbye` use the control reserve.
- `Error` and `StreamEnd` use the terminal reserve if their bodies fit that
  class.
- Everything else, including a channel-0 `Request`, is ordinary and never
  bypasses.

A control frame other than `Goodbye` passes a blocked ordinary head at once.
A terminal frame passes only when no earlier pending frame shares its
`(channel, corr)`. A stream's data therefore always precedes its end.
`Goodbye` waits for every earlier frame. A frame past its deadline retires as
`not_sent` without publishing.

Each connection holds 63 terminal credits. A request takes one before dispatch.
The credit returns when the terminal's block physically returns. Admitted
requests cannot exceed the terminal inventory, and one block stays free for
a pre-admission rejection.

The managed Rust client (`crates/host-runtime/src/client.rs`) and the native
addon behind the TypeScript client (`packages/shm-native`) use the same rule.
Both classify inventory from the wire header before a nonblocking reservation.
They keep data frames in admission order and let only a liveness `Pong` bypass
waiting data. When an inventory is exhausted, they park on the capacity
doorbell using section 8's arm-and-recheck protocol. Host consumption or return
provides the wake they need.
