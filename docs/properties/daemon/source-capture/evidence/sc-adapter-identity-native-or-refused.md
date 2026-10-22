# sc-adapter-identity-native-or-refused

Abbreviations follow [`../catalog.md`](../catalog.md): `hs.rs`, `hs-tests`,
`sc.rs`, and `sc-tests` name the harness-source and source-capture modules
and their tests.

## Discovery trigger

Part A record of the source-capture catalog (spec #1066 comment 1). R4 requires
native identity or refusal. U3 changed the Pi `type` arm the baseline record
flagged, so the record was re-verified at HEAD `5a842d998`.

## Evidence trail

- `crates/daemon/src/harness_sources.rs` `opencode_units`: refuses a non-OpenCode
  binding (`UnsupportedShape("harness")`), non-object `info` (`NotAnObject`),
  missing `id`, `sessionID`, `role` (`MissingIdentity`), a foreign session
  (`SessionMismatch`), roles other than `user`/`assistant`
  (`UnsupportedShape("info.role")`), missing `time` (`MissingIdentity("time")`),
  non-array `parts`, and a settled tool without `callID` or `state.time.end`.
  Identity values come from `info.id`, the bound `SessionIdentity`, `callID`,
  and `state.time.end` through `field` and `revision`.
- `pi_units`: refuses a missing, null, or empty `type` as
  `MissingIdentity("type")` and any other value as `UnsupportedShape("type")`,
  then reads `id`, `message`, `message.role`, `message.timestamp`, and for tool
  results `parentId`, `toolCallId`, and boolean `isError`.
- `SessionIdentity::message_unit` and `tool_unit` build the identity vectors in
  class field order: `project_id, harness, session_id, message_id, block_index`
  and `project_id, harness, session_id, parent_message_id, tool_call_id,
  result_revision, block_index`.
- The 512-byte identity cap is enforced by `SourcePublisher::prepare` through
  `encode_preserving_span`, not by the adapters.
- Production caller: `CaptureJob::units` in
  `crates/daemon/src/source_capture.rs` calls the adapter for the session's
  harness and counts each `Err` as `adapter_refused`. The job starts from
  `Handler::capture_after_response` in `crates/daemon/src/lib.rs` only when
  `source_capture::consents` is true; `DaemonConfig::default()` sets
  `source_capture_enabled: false`, a `TierClass::UserOnly` key.
- Tests read at HEAD:
  - `crates/daemon/tests/harness_sources.rs`
    `adapters_bind_native_identity_exactly_and_refuse_missing_identity`
    asserts each identity tuple and every refusal arm, including six Pi rows
    for the `type` arms (comment names witness `pi_wire_row_without_type`).
  - `crates/daemon/tests/eval_ingestion.rs`
    `each_adapter_identity_field_flips_its_ids_and_payload_fields_flip_none`
    flips ten fields; identity fields move the ids they enter, and `text`,
    `role`, and `tool output` move none.
  - `crates/daemon/src/source_capture_tests.rs`
    `pi_entry_identity_on_the_row_decides_capture_and_never_the_response`
    strips `type` and `parentId` from three rows and asserts no tool
    descriptor and `adapter_refused == 3`.

## Failure scenario

An adapter reads an identity value from text, a tool name, or a path, or
synthesizes a missing one. Two native blocks with equal bytes then share one
occurrence, or one block gets a new occurrence on each pass. Falsifier: a unit
returned for a record lacking `callID`, or an occurrence id that changes when
only the text changes.

## Timing windows and dependencies

None. Each adapter call is a pure function of one record and the bound session.
The capture path depends on the plugin sending rows with `type` and `parentId`
(U3 TypeScript change).

## What a test must construct

- One record per refusal arm, asserting `Err` and no unit.
- A field-flip matrix over identity and payload fields, asserting which ids
  move.
- A capture-path pass with rows lacking identity, asserting
  `adapter_refused` and no descriptor.

All three exist at HEAD.

## Investigation log

### Q: Disposition name for a Pi row without `type` (OQ10)
- Sources examined: `ticket-1083.md` (Resolved OQ10), `pr-1083.md`, `pi_units`,
  `adapters_bind_native_identity_exactly_and_refuse_missing_identity`.
- Findings: U3 reuses existing arms: `MissingIdentity("type")` when absent,
  null, or empty, `UnsupportedShape("type")` otherwise. The baseline silent
  `Ok(vec![])` arm no longer exists.
- Missing evidence: none.
- Conclusion: resolved with answer; landed in U3.

### Q: Is reachability still `test-only`?
- Sources examined: `grep` for `opencode_units(` and `pi_units(` under
  `crates/daemon/src`; `source_capture::consents`; `config.rs`.
- Findings: the only non-test caller is `CaptureJob::units`, gated by
  `consents`.
- Missing evidence: none.
- Conclusion: resolved with answer: `explicit-config-only`.

### Q: Do the baseline line citations still hold?
- Sources examined: `hs-tests` at HEAD.
- Findings: baseline `hs-tests:442-731` and the listed lines moved; the record
  names tests instead. `hs.rs:324-326` (silent `Ok(vec![])`) was replaced by
  the `type` match in `pi_units`.
- Missing evidence: none.
- Conclusion: resolved with answer: citations replaced by function and test
  names.
