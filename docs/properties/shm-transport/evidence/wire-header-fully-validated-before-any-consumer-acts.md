# wire-header-fully-validated-before-any-consumer-acts

## Citation refresh, 2026-08-30

The ring-transport refactor (`0f336d3c`, `d8bde128`, `793a973e`, `ed487e11`)
renamed `crates/host-runtime/src/shm_provider.rs` to
`crates/host-runtime/src/ring_transport.rs` and deleted `provider_recovery.rs`,
`transport_negotiation.rs`, and `transport_provider.rs`. Host-side citations below
were re-anchored against `ring_transport.rs` at `e447c927`.

Where the cited construct survives, the citation names `ring_transport.rs` and a
line re-verified against that commit. Where it does not, the original reference is
kept and prefixed `former`, so it reads as pre-refactor evidence rather than a
current location. A `former` line number is never a claim about the tree today.
Every `provider_recovery.rs` reference is `former` by definition: that module has
no successor. See the refresh note in [../catalog.md](../catalog.md).

## Discovery trigger

`FrameDescriptor::validate` carries a 21-byte `wire_header`
(`crates/shm-transport/src/descriptor.rs:208`, length constant at `:19`) and
inspects exactly five of those bytes (`:32-40`). The other sixteen are copied
into `ValidatedFrame` (`:326`) and handed to a consumer untouched. The transport
crate cannot inspect them even in principle: `crates/shm-transport/Cargo.toml`
depends on `getrandom`, `libc`, `serde`, and `iceoryx2`, while
`crates/host-runtime/Cargo.toml:25` depends on the transport, so the dependency edge
runs host-to-transport and `FrameType`, `Flags`, and `PROTOCOL_VERSION` are
unreachable from the validating crate. The interesting property is therefore not
either layer's check list but the composition: which layer owes which field, and
whether the owed check runs before anything acts on the frame.

## Evidence trail

Transport side, receiver direction. `Ring::try_receive` snapshots the shared
descriptor with one `read_volatile` (`backend/ring.rs:1440`), validates it
(`:1444`), and on failure quarantines the ring and returns
`RingError::Descriptor` (`:1445`). Inside `validate`, the only header checks
are `declared_len` from bytes 0..4 against `body_len`, and `wire_header[4] != 2`,
both yielding `DescriptorError::WireHeaderMismatch` (`descriptor.rs:32-40`,
variant documented at `:479`). Bytes 5 through 20 — type, flags, channel, epoch,
correlation — are never read. The same two checks appear on the producer side in
`commit_reservation` (`ring.rs:2316-2317`, `ProducerError::WireHeaderMismatch`)
and, at `9c1eb4d1`, a third time in the iceoryx backend
(`backend/iceoryx.rs:257-262`), which `0f336d3c` deleted. The
literal `2` in all three places mirrors `PROTOCOL_VERSION` (`wire.rs:25`) with no
shared definition, and `MAX_FRAME_BYTES` (`arena.rs:4`) mirrors
`MAX_FRAME_BODY_LEN` (`wire.rs:38`); both pairs are 64 MiB today and neither pair
is cross-checked.

Host side. `decode_header` (`crates/host-runtime/src/wire.rs:323-382`) reads the
frozen prefix, dispatches header length on `ver` through `header_len_for_version`
(`:313-318`), then rejects an unknown type byte (`:337-338`), reserved flag bits
(`:340-342`), a reserved priority (`:343-345`), a reserved admission class
(`:346-348`), `Sheddable` on a type other than `Push`/`StreamData` (`:349-356`),
a pure-header type with a nonzero length (`:357-359`), and the two channel/epoch
cross-rules (`:360-368`). Correlation (`:369-371`) is accepted unconditionally.
`validate_inbound_header` (`frame_channel.rs:41-59`) then adds the 64 MiB body
cap, the pure-header flag rule, and the consumer-role type whitelist.

Ordering. `receive_one` (`ring_transport.rs:664-747`) runs `try_receive` (`:676`),
`decode_header` (`:681`), `validate_inbound_header` (`:683`), the channel-0
control cap (`:684-696`), the ingress charge loop (`:698-730`), the body copy
(`:731`), the completion (`:734-736`), and the send to the consumer (`:737-745`), in
that order. Nothing between `:676` and `:683` reads payload bytes, so both header
gates precede every charge, copy, and dispatch. The obligation was stated in the
pre-#131 doc comment on `validate_inbound_header` ("Classification uses the
header alone, BEFORE any body admission", former `frame_channel.rs:53-57`); the
PR #131 comment trim reduced that comment (`frame_channel.rs:48-49` (source tree; not at HEAD) at HEAD) and
the explicit statement is gone — the ordering obligation now rests on the code
alone. A
role-invalid type with a large declared body must not hold ingress budget or an
allocation through the frame deadline. That deadline is real — the charge wait at
`:702-730` can spin until `frame_deadline`.

Rejection consequence. Both gates map to `ReadClose::Corrupt`
(`ring_transport.rs:682-683`, variant at `frame_channel.rs:32`). `Corrupt` is not
in the clean set (former `shm_provider.rs:498`), so `run_endpoint` returns `false` and
the spawn wrapper takes `recovery.report_suspect(custody)` instead of
`custody.release()` (former `:364-371`). `report_suspect`
(former `provider_recovery.rs:360-397`) starts a recovery episode whose `cleanup` may
answer `Reclaimed`, `StaleRetry`, or `Uncertain` (former `:94-103`); only `Uncertain`
isolates. A header rejection therefore does not quarantine directly — it closes
the generation and hands the decision to the controller. The receive lease drops
unreleased and releases itself (`lease.rs:366-372`).

The documented transport contract does not assign these fields to anyone:
`docs/shm-transport.md:13` (source tree; not at HEAD) says the receiver "snapshots and validates
descriptor metadata", and the header's own fields are not mentioned.

## Failure scenario

Three drifts break the composition without breaking either layer's tests. First,
moving the charge or the copy above `ring_transport.rs:731` gives an attacker-declared length the
power to hold up to 64 MiB of ingress budget for a full frame deadline on a frame
whose type is already known-illegal — the exact outcome the doc comment forbids.
Second, a consumer that reads `ValidatedFrame::wire_header()` without running
both host gates acts on sixteen unvalidated bytes; the transport's success return
is not evidence about them. Third, a version 3 that relocates `len` or `ver`
inside the header — the extension point at `wire.rs:313-318` exists precisely to
allow this — leaves `descriptor.rs:32-38` validating whatever now occupies
offsets 0..5, silently, since the transport cannot see the version registry.

## Timing windows and dependencies

No interleaving is required; the property is an ordering over one synchronous
path, and it holds at HEAD. The window that makes it load-bearing is the charge
loop (`ring_transport.rs:702-730`), bounded by `frame_deadline`: anything moved
above it inherits that hold time. Depends on
`receive-failure-leaves-no-wedged-slot` for the slot state after a rejection, and
on `quarantine-authority-survives-peer-writes` for the premise that a peer can
author descriptor bytes at all — both mappings are `PROT_READ|PROT_WRITE`
(`ring.rs:462`, `:481`).

## What a test must construct

A hostile producer that writes the shared descriptor page directly, because the
producer API cannot express these frames: `TestShmPeer::send`
(`ring_transport.rs:885-892`) builds the header with `EnvelopeHeader::encode`
(`wire.rs:204-214`) and commits `body.len()`, so `commit_reservation` rejects any
length disagreement before publication. With direct page authorship, one frame
per field class — unknown type byte, reserved flag bit 6 or 7, reserved priority
`0b11`, reserved admission `0b11`, `Sheddable` on `Request`, pure-header with a
body, nonzero epoch on channel 0, zero epoch on a routed channel, role-invalid
type — asserting for each that the close is `Corrupt`, that the ingress budget's
used-byte count is unchanged across the rejection, and that no
`InboundEvent::Frame` was emitted. The last two assertions are what pin the
ordering; asserting only the rejection would survive a reordering.
The pre-#131 `crates/host-runtime/tests/shm_failure_modes.rs:195-241` covered one
field (type) end to end, including the then-existing suspect-and-isolate tail;
that test is absent from the rewritten post-#131 file, so the end-to-end arm
must be rebuilt rather than generalised. A static counterpart is worth more than a fault harness here: assert
that every reader of `ValidatedFrame::wire_header()` outside a test reaches
`decode_header` and `validate_inbound_header`.

## Investigation log

### Q: Can a rejected header hold ingress budget or a receive lease past the rejection?

- Sources examined: former `shm_provider.rs:555-618` for the full order,
  `ring_transport.rs:702-730` for the charge loop, `lease.rs:366-372` for lease drop,
  `frame_channel.rs:48-49` (source tree; not at HEAD) for the (now trimmed) doc comment; the documented
  obligation itself survives only in the pre-#131 comment quoted above.
- Findings: no. The charge is acquired at `ring_transport.rs:700`, strictly after both gates, and
  the only earlier resource is the receive lease itself, which `Drop` releases.
  The `?` at `ring_transport.rs:682` and `ring_transport.rs:683` returns before `deadline` is even computed
  (`ring_transport.rs:698`), so the frame-deadline hold cannot be reached by an invalid header.
- Missing evidence: the control-cap branch at `ring_transport.rs:684-696` releases the lease
  explicitly and answers `Rejected` rather than closing, which is a fourth
  disposition beyond accept, `Corrupt`, and drop. Whether a peer can profit by
  parking correlations through that branch was not investigated.
- Conclusion: the ordering claim is established at HEAD by direct read. The
  record is about keeping it, not about a live defect.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 22, `:265-273` now `:32-40`: At HEAD the two checks live in the shared `check_wire_header` helper (`descriptor.rs:28-42`), which `validate` calls at `:323`.
  - line 35, `backend/ring.rs:1093` now `backend/ring.rs:1440`: At HEAD `try_receive_inner` snapshots with `slot.read_descriptor()` (`ring.rs:1440`), whose single `read_volatile` sits in `DescriptorSlot::read_descriptor` (`:198`).
  - line 37, `:1097-1100` now `:1445`: `try_receive_inner` only maps the error (`ring.rs:1445`); the quarantine happens one level up in `try_receive` (`:1399-1401`).
  - line 42, `ring.rs:1585-1593` now `ring.rs:2316-2317`: The producer side is `prepare_commit` (`ring.rs:2308-2317`) calling the same `check_wire_header` helper, so the two checks exist once rather than twice.
  - line 45, `wire.rs:24` now `wire.rs:25`: A shared definition exists at HEAD: `check_wire_header` compares against `WIRE_V2_VERSION` (`descriptor.rs:21`), and `wire.rs` re-exports `PROTOCOL_VERSION` (`:25`) under a const assertion that the two agree (`:27-30`).
  - line 47, `wire.rs:31` now `wire.rs:38`: `MAX_FRAME_BODY_LEN` is derived from `shm_transport::MAX_FRAME_BYTES` and guarded by a const assertion (`wire.rs:38-43`), so this pair is cross-checked rather than mirrored.
  - line 112, `ring.rs:321` now `ring.rs:462`: Both mapping sites call `sys::mmap_shared`, which passes `PROT_READ | PROT_WRITE` and `MAP_SHARED` (`backend/sys.rs:94-95`).
  - line 118, `ring_transport.rs:684-698` now `ring_transport.rs:885-892`: The type is `RingClientEndpoint` at HEAD, and `send` delegates to `send_bounded` (`ring_transport.rs:901-933`), which hands `header.encode()` to `reserve_until` and commits `body.len()`.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 68, `frame_channel.rs:48-49` (trimmed doc comment on validate_inbound_header): No doc comment survives on `validate_inbound_header` at HEAD; the function begins at `frame_channel.rs:41` and the ordering obligation is unstated.
  - line 87, `docs/shm-transport.md:13` (doc sentence on the receiver snapshotting and validating descriptor metadata): That sentence no longer appears in `docs/shm-transport.md`; the setup phase list states only that validation covers the profile, wire version, descriptor schema, grants, and activation token (`:43`).
  - line 141, `frame_channel.rs:48-49` (trimmed doc comment): `validate_inbound_header` (`frame_channel.rs:41-59`) carries no doc comment at HEAD.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
