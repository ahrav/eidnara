# `addon-grant-decoding-is-the-shared-setup-envelope`

- **Discovery:** U3, when the addon setup path was catalogued.
- **Primary evidence:** the addon's `GrantMessage` (`packages/shm-native/src/setup.rs`) is a `serde` enum tagged by `type` with `deny_unknown_fields`, one `grant` variant, and a nested `Descriptor` of `profile`, `host_to_peer_grant`, and `peer_to_host_grant`. The host's `GrantMessage` (`crates/host-runtime/src/setup_socket.rs`) is a struct tagged `grant` whose `descriptor` is an untyped `serde_json::Value`. The two definitions are written independently; no shared type, schema, or fixture links them. `peer_closed` uses `MSG_PEEK | MSG_DONTWAIT` on the setup socket so a closed host is reported without consuming bytes.
- **Existing evidence:** `grant_message_accepts_tagged_setup_envelope` decodes an addon-local JSON literal and asserts the wire version. `peer_closed_reports_live_then_dropped_sentinel` asserts a held socket is live and a dropped host end is closed. The host's `grant_transfers_exactly_six_descriptors_close_on_exec` and the mismatch tests build their own `GrantMessage` with placeholder descriptors. The transport fuzz corpus (`crates/shm-transport/fuzz/fuzz_targets/provider_grant.rs`) exercises the binary `RingGrant` decoder, not this JSON envelope.
- **Failure scenario:** a field rename or tag change on one side leaves both unit suites and the fuzz corpus green while every addon connection fails setup, or a host-issued grant with an extra field is refused by `deny_unknown_fields`.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** the addon-side decode and the dropped sentinel are proven; cross-side agreement is not. Confidence is `low` until a committed fixture or a live host-to-addon setup test pins the two definitions together.
- **Open-question log:** add a shared fixture or a live host-to-addon test (needs human input).
