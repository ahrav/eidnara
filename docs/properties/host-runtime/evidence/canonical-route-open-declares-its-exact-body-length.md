# `canonical-route-open-declares-its-exact-body-length`

- **Discovery:** U3, when the module id in the canonical body was renamed.
- **Primary evidence:** `canonical_route_open_body_is_167_bytes` counts the literal body; `committed_header_vectors_decode_to_their_documented_fields` decodes the committed header with the test-local `raw_client::decode_header` and re-encodes with `raw_client::header`. Section 6.4 of `docs/host-wire-protocol.md` carries the same bytes.
- **Existing evidence:** the two tests named above.
- **Failure scenario:** declared length disagrees with the body.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The oracle is the test-local decoder, not the crate's; the length is asserted on the literal string, so a body edit that forgets the header is caught.
- **Open-question log:** none.
