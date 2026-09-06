# canonical-route-open-declares-its-exact-body-length

## Discovery trigger

The module id in the canonical `route.open` body was renamed to `context` at
U3, which shrank the body from the predecessor's length and forced the
committed control header to be regenerated once. The header's `len` field is
what the reader trusts for framing, so a header that declares a length other
than the body it precedes desynchronizes every frame that follows. The audit
checked that the committed header bytes decode to the documented fields, that
the canonical body is exactly the declared length, and that the oracle doing
the decoding is not the crate's own decoder.

## Evidence trail

All references are at `572315a`.

Documented vector. `docs/host-wire-protocol.md:315-324` (section 6.4)
states the compact canonical `route.open` request is 167 UTF-8 bytes and
gives the header `a7 00 00 00 02 00 02 00 00 00 00 00 00 01 00 00 00 00 00
00 00`, hex `a70000000200020000000000000100000000000000`. The body literal
is at `:360` (section 7.2). The header layout is at `:237-249`: `len`
at offset 0 as little-endian `u32`, `ver` at 4, `type` at 5, `flags` at 6,
`channel` at 7, `epoch` at 9, `corr` at 13, 21 bytes total.

Independent oracle. `tests/support/raw_client.rs:1-7` states the module
reimplements framing from the protocol's literal values and never calls the
crate's encoders. `HEADER_LEN` is 21 (`:21`), `TY_REQUEST` is 0 (`:27`),
`FLAGS_INTERACTIVE` is `0b0000_0010` (`:41`). `header` at `:271-281` writes
`len` little-endian, then version, type, flags, channel, epoch, and
correlation. `decode_header` at `:283-295` asserts 21 bytes and reads the
same layout back.

Production reader. The crate's `wire::decode_header` (`wire.rs:311`) reads
`len` from bytes 0..4 (`:324`) after the frozen-prefix check (`:312-322`).
Over the shared-memory transport, `ring_transport.rs:500-501` decodes the
lease's wire header, `:503` compares `header.len` against
`MAX_CONTROL_BODY_LEN` (65,536, `wire.rs:374`) for control requests, and
`:517` charges ingress by `header.len`. The body bytes come from the lease
(`:540-542`). The transport layer enforces that the declared length equals
the lease body length: `check_wire_header`
(`crates/shm-transport/src/descriptor.rs:27-42`) rejects a descriptor whose
`wire_header[0..4]` differs from `body_len` with `WireHeaderMismatch`, and
is called from the ring backend at `backend/ring.rs:2333`, the sample path
at `backend/sample.rs:93`, and `descriptor.rs:322`.

Existing checks, verified, both in `tests/protocol_vectors.rs`, a
default-harness binary CI runs via `cargo test --workspace --all-targets`
(`.github/workflows/ci.yml:118`):

- `committed_header_vectors_decode_to_their_documented_fields`
  (`:160-193`) converts the documented hex to bytes (`:163`), asserts 21
  bytes (`:164`), decodes with the test-local `decode_header` (`:165`), and
  asserts `len == 167`, `ver == 2`, `ty == TY_REQUEST`,
  `flags == FLAGS_INTERACTIVE`, `channel == 0`, `epoch == 0`, `corr == 1`
  (`:166-172`). It then re-encodes with `raw_client::header(167, ...)` and
  asserts equality with the committed bytes (`:173-177`). The routed
  44-byte header on channel 7, epoch 77, correlation 2 is checked the same
  way (`:181-192`).
- `canonical_route_open_body_is_167_bytes` (`:196-218`) builds the body as a
  `concat!` of two string literals (`:197-200`), asserts `canonical.len()
  == 167` (`:201-205`), converts the committed control header hex to bytes
  and asserts `raw_client::decode_header(&control).len == canonical.len()`
  (`:208-213`), parses the body as JSON (`:215`), and asserts
  `op == "route.open"` and `target.module_id == LINKED_MODULE_ID`
  (`:216-217`). `LINKED_MODULE_ID` is `"context"` (`tests/support/mod.rs:25`).

The body test ties the two literals together: the header's decoded length
field is compared with the body's own length, so the header cannot declare a
length the body does not have.

## Failure scenario

1. The canonical body is edited, for example the session literal changes
   from `session-1` to `session-01`, making it 168 bytes.
2. The committed header still declares `a7` (167). A reader that trusts the
   header reads 167 body bytes and then treats the trailing `}` as the first
   byte of the next header.
3. `canonical_route_open_body_is_167_bytes` fails twice: on
   `canonical.len() == 167`, and on the decoded header length, which still
   reads 167 while the body is 168. Updating the body's expected length to
   168 without re-encoding the header leaves the second assertion failing,
   so the header and the body cannot be edited independently. Section 6.4 of
   the protocol document is a third copy of the same numbers that no test
   reads.

## Timing windows and dependencies

None. Both tests are pure literal comparisons. The property depends on the
`HEADER_LEN` and field offsets in `raw_client.rs`, which are the oracle's
own constants rather than the crate's, and on section 6.4 of the protocol
document staying in step with the tests. Nothing in the tree reads the
document's hex; the tests carry their own copy.

## What a test must construct

The record's check is `canonical.len() == 167`,
`raw_client::decode_header(committed).len == canonical.len()`, and
`raw_client::header(167, ...) == committed bytes`, all of which exist. One
addition would strengthen the record:

1. A live-connection check that sends the canonical body under the committed
   header through `RawClient` and receives a `route.open` response, proving
   the host frames exactly 167 bytes. `structural_corruption_is_rejected_
   before_dispatch` (`:362`) covers wrong headers but not this exact vector.

## Investigation log

### Q: Is the decoder that checks the header independent of the crate's?

- Sources examined: `tests/support/raw_client.rs:1-7`, `:21-41`,
  `:271-295`; `wire.rs:311-324`; the `use` list at
  `tests/protocol_vectors.rs:12-16`.
- Findings: the test file imports `decode_header` and `header` from
  `support::raw_client`, not from `host_runtime::wire`. The oracle's layout
  constants are its own. The crate's `decode_header` reads the same offsets,
  and the two agree on this vector because both follow the protocol table at
  `docs/host-wire-protocol.md:241-249`.
- Missing evidence: none.
- Conclusion: resolved. The oracle is test-local, as the record states.

### Q: Does the managed client emit the canonical body byte for byte?

- Sources examined: `client.rs:2175-2211`; the workspace `Cargo.lock` entry
  for `serde_json`; `docs/host-wire-protocol.md:360`.
- Findings: `route_open_body` builds the request with `serde_json::json!`
  (`client.rs:2187-2194`) and serializes with `serde_json::to_vec` (`:2211`).
  The workspace does not enable serde_json's `preserve_order` feature, so
  object keys serialize in sorted order: `identity`, `op`, `target`. The
  documented literal is in `op`, `target`, `identity` order. Reordering the
  keys of the same object yields the same 167 bytes, so the declared length
  still holds for what the client sends, but the client's body is not the
  documented literal's byte sequence. A test that compared the client's
  bytes to the literal would fail; no test does.
- Missing evidence: none for the length property. Whether the protocol
  intends the literal to be byte-exact or only length-exact is not stated
  in section 7.2.
- Conclusion: resolved for the length. The key-order question needs human
  input if section 7.2 is meant to fix the byte sequence rather than the
  JSON value.
