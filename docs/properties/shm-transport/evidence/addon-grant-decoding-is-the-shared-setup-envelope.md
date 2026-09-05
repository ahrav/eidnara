# addon-grant-decoding-is-the-shared-setup-envelope

## Discovery trigger

The host and the addon each define the setup grant envelope independently. A field or tag change on one side would leave both unit suites and the transport fuzz corpus green while every addon connection failed setup.

## Evidence trail

- The addon's `GrantMessage` (`packages/shm-native/src/setup.rs:41-50`) is a `serde` enum tagged by `type` with `deny_unknown_fields`, one `grant` variant, and a nested `Descriptor` of `profile`, `host_to_peer_grant`, and `peer_to_host_grant`.
- The host's `GrantMessage` (`crates/host-runtime/src/setup_socket.rs:58-66`) is a struct tagged `grant` whose `descriptor` is an untyped `serde_json::Value`.
- `peer_closed` (`setup.rs:187`) uses `MSG_PEEK | MSG_DONTWAIT` so a closed host is reported without consuming bytes.
- `grant_message_accepts_tagged_setup_envelope` (`setup.rs:597`) decodes an addon-local JSON literal; `peer_closed_reports_live_then_dropped_sentinel` (`setup.rs:751`) asserts a held socket is live and a dropped host end is closed.
- The host's `grant_transfers_exactly_six_descriptors_close_on_exec` and mismatch tests build their own `GrantMessage` with placeholder descriptors.
- The transport fuzz corpus (`crates/shm-transport/fuzz/fuzz_targets/provider_grant.rs`) exercises the binary `RingGrant` decoder, not this JSON envelope.

## Failure scenario

A field rename or tag change on one side, or a host-issued grant with an extra field that `deny_unknown_fields` refuses. Both suites stay green; every real connection fails.

## Timing windows and dependencies

None.

## What a test must construct

- Present: the addon-side decode and the dropped sentinel.
- Missing: a committed fixture the host serializes against and the addon deserializes, or a live host-to-addon setup test.

## Investigation log

### Q: Is there any artifact that pins the two definitions to one another?

- Sources examined: Both `GrantMessage` definitions, both test suites, the fuzz targets.
- Findings: No shared type, schema, or fixture; agreement is by construction only.
- Missing evidence: The shared fixture or live test.
- Conclusion: needs human input: choose a fixture or a live test; confidence stays `low` until one exists.
