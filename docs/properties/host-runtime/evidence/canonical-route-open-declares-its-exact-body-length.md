# `canonical-route-open-declares-its-exact-body-length`

- **Discovery:** U3, when the module id in the canonical body was renamed.
- **Primary evidence:** `canonical_route_open_body_is_167_bytes` counts the literal body and decodes the committed control header with the test-local `raw_client::decode_header`, asserting the header's length field equals `canonical.len()`; `committed_header_vectors_decode_to_their_documented_fields` decodes the same header field by field and re-encodes it with `raw_client::header`. Section 6.4 of `docs/host-wire-protocol.md` carries the same bytes.
- **Existing evidence:** the two tests named above.
- **Failure scenario:** declared length disagrees with the body.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The oracle is the test-local decoder, not the crate's. The body test compares the decoded header length with the body's own length, so a body edit whose expected length is updated without re-encoding the header fails; the two literals were compared only to each other's copies of `167` before that assertion was added.
- **Open-question log:** none.
