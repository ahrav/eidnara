# sc-ready-snapshot-drops-native-messages

## Discovery trigger

Part A record of the source-capture catalog (#1066 comment 1), written at
baseline `08de9051a` for D1: the ready snapshot drops the native window, so a
background publisher must hold its own charged reference. U2
(`ticket-1082.md`) lists this record as unchanged. This file re-verifies it at
HEAD `5a842d998`.

## Evidence trail

`crates/daemon/src/lib.rs`:

- `TransformRequest::ready_snapshot` clones the request with
  `native_messages: None` and `coverage` replaced by
  `coverage.resolution_only()`. The clone shares the `messages` `Arc`s.
- `TransformRequest::snapshot_retained_bytes` documents that a snapshot never
  holds native messages, so none are charged.
- `TransformSnapshotCache::finish_ready` starts with
  `debug_assert!(request.native_messages.is_none())`. Release builds check
  nothing at that point.
- The transform handler builds `Arc::new(parsed.ready_snapshot())`, charges
  `snapshot_retained_bytes`, and calls `finish_ready`.
- Earlier in the same handler, when `source_capture::consents` holds, the pass
  builds a `source_capture::Offer` whose `window` is
  `parsed.native_messages.clone()`, a vector of the same `Arc<Value>`s.

`crates/daemon/src/source_capture.rs`:

- `SourceCapture::offer` sums `retained_size::value_retained_bytes` over the
  window, doubles it with the document bytes, adds
  `ARTIFACT_PREPARATION_BYTES`, refuses above `MAX_RETAINED_WINDOW_BYTES`, and
  takes a `RetainedCharge` for the `CaptureHandoff`. The native window lives in
  the handoff under that charge, independent of the snapshot.
- `consents` requires `source_capture_enabled`, `memory_enabled`, a
  non-subagent pass, a native-serving profile, and a matching route harness.

Tests read at HEAD:

- `lib.rs::ready_snapshot_shares_messages_and_charges_at_least_their_bytes`
  sets `native_messages: Some`, asserts the snapshot's is `None`, asserts
  `Arc::ptr_eq` on `messages[0]`, and asserts the charge grows with message
  bytes.
- `lib.rs::ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation`
  sends a native request with a 64 KiB `provider_data` field through the
  handler and asserts the lease's request has no native messages and a charge
  under 64 KiB.
- `transform/surface_census.rs::auto_search_is_enabled_by_the_wire_default_and_drives_the_hint_query`
  asserts a request on a non-native serializer profile parses with no native window; it does not touch
  the snapshot.

## Failure scenario

The snapshot keeps the native payload alive past the response, so the ready
LRU and active-lease budget under-charge the bytes they pin. In release
builds a regression in `ready_snapshot` passes `finish_ready` silently.

## Timing windows and dependencies

None for the snapshot. The capture handoff holds its own clone of the native
`Arc`s from offer until the job drops it; that lifetime is a separate record
(`sc-one-outstanding-batch-refuse-newest`, `sc-response-independent-of-publication`).

## What a test must construct

A request with `native_messages: Some` on the `opencode-aisdk` or `pi`
profile; a pass that reaches ready; a large native payload so an under-charge
is visible.

## Investigation log

### Q: Did U2 make the snapshot hold the native window?

- Sources examined: `ready_snapshot`, `finish_ready`, the transform handler's
  `Offer` construction, `SourceCapture::offer`.
- Findings: no. U2 retains the native window through `Offer::window` and the
  `CaptureHandoff` charge. The snapshot still drops it.
- Missing evidence: none.
- Conclusion: resolved with answer; the record stays active.

### Q: Do the baseline citations still hold?

- Sources examined: `lib.rs` at HEAD and at `08de9051a`.
- Findings: baseline `lib.rs:23483` was the start of
  `ready_snapshot_shares_messages_and_charges_at_least_their_bytes`, now at
  `:23773`. The `debug_assert!` moved from `:2135` to `:2137`. `ready_snapshot`
  is at `:1828`. The handler call site moved from baseline `:9825-9835` to the
  `Arc::new(parsed.ready_snapshot())` line at `:9974`.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Should release builds enforce the absence?

- Sources examined: `finish_ready`.
- Findings: only a `debug_assert!` guards it.
- Missing evidence: a defensive-assertion review.
- Conclusion: unresolved, needs
  `/low-level-systems:defensive-assertions-and-invariant-guards`.
