# sc-outcome-observed-bounded-occurrence-bound

## Discovery trigger

Part B obligation from #1066 comment 2, requirement R8 and decisions D7 and
D8. The U5 ticket (#1085) names the record with four faults (completed with
exit 1, error without exit, absent metadata, malformed metadata) and resolves
OQ3 through a pinned OpenCode trace fixture captured in that unit.

## Evidence trail

`crates/kernel/src/source_descriptor.rs` - `ToolOutcome` holds `version`,
`status` (`ToolStatus::Completed` or `Error`), `tool_name`, `byte_length`,
and optional `truncated`, `exit_code`, and `exception_ref`; the optional
fields use `skip_serializing_if = "Option::is_none"`, and the struct is
`deny_unknown_fields`. `ToolOutcome::validate` requires
`version == TOOL_OUTCOME_VERSION`, `status == Error` exactly when the
representation is `tool_error`, a well-formed tool name and exception name
that `identity` leaves unchanged, `byte_length == selected_len`, and
`encode().len() <= MAX_TEXT_FIELD_BYTES` (1024, `crates/kernel/src/cas/mod.rs`).
`check_outcome` refuses `OutcomeRefused` for a record on any class other than
`raw_tool_spans` or one that fails `validate`. `SourceDescriptorRequest`
carries `outcome: Option<&ToolOutcome>` and `SourceDescriptorDetail` stores
`outcome: Option<ToolOutcome>` beside the identity, outside
`occurrence_tuple`.

`crates/daemon/src/harness_sources.rs` - `SourcePublisher::prepare` calls
`kernel::check_outcome(unit.outcome, unit.class, representation,
unit.text.len())` before any byte is retained, so an invalid record refuses
`PublishError::Descriptor { refusal: OutcomeRefused, evidence: None }`.

`crates/daemon/src/tool_outcome.rs` - `opencode` reads `status` from
`state.status`, `truncated` from a boolean `state.metadata.truncated`,
`exit_code` from an integer `state.metadata.exit` only when the tool is
`bash`, and `exception_ref` from `info.error.name` only when
`state.metadata.interrupted` is `true` and the name passes
`is_exception_name`. `pi` reads `status` from a boolean `isError` and
`truncated` from `details.truncation.truncated`, and sets neither
`exit_code` nor `exception_ref`. Absent or wrongly typed fields map to
`None`.

Landed checks:
`crates/daemon/tests/harness_sources.rs::opencode_tool_outcomes_follow_the_pinned_trace`
asserts every case of `crates/daemon/testdata/opencode-tool-outcome-trace.json`
(OpenCode 1.19.0: exit 1, exit 0 truncated, null exit, error without
metadata, error with output-only metadata, interrupted by abort, `read`) maps
to its recorded outcome, and that a string `exit`, absent metadata, and a
non-`bash` `exit` give no exit code.
`crates/daemon/tests/harness_sources.rs::pi_tool_outcomes_read_status_and_reported_truncation`
covers the Pi mapping.
`crates/daemon/tests/harness_sources.rs::equal_tool_bytes_with_different_outcomes_are_two_descriptors_of_one_artifact`
publishes a completed `bash` call with exit 1 and an error call with the same
bytes and asserts two descriptors with one `artifact_digest` and records
`(Error, None)` and `(Completed, Some(1))`.
`crates/daemon/tests/harness_sources.rs::an_invalid_outcome_record_is_refused_before_retention`
asserts a record over 1 KiB, a wrong byte length, a record on a message, and
an empty tool name refuse with the tip and staged artifacts unchanged, and
that `publish_screened` refuses only that unit.
`crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record`
asserts the stored record, `occurrence_tuple` equal to the encoding without a
record, acceptance at exactly 1024 bytes, and refusal of the other class,
whole-buffer length, version 2, a secret tool name, 1025 bytes, and a status
that disagrees with the representation.
`crates/kernel/tests/eval_identity.rs::the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder`
pins the identity goldens; `git diff 04b60e8e0..b953b32b9` shows no change to
that file or its fixtures in U5.

## Failure scenario

An extractor that defaults a missing `exit` to `0` records success for a
failed call. An outcome folded into the artifact or the identity makes equal
bytes from two calls collide, or moves every published occurrence id.

## Timing windows and dependencies

None in the record itself. An interrupted OpenCode call is read only once
its turn has `time.completed` (`opencode_interrupted` and the adapter), so
the exception name is final when the record is built;
`an_interrupted_call_waits_for_its_turn_to_complete` covers that ordering.

## What a test must construct

The four-input matrix over the real adapters, a published pair of equal-byte
calls with different outcomes, the refusal set before retention, and the
identity goldens. The legal witness is an error part without exit yielding
`status: error` and no `exit_code`.

## Investigation log

### Q: Which `state.metadata` key carries the exit code (OQ3)
- Sources examined: `ticket-1085.md`; `tool_outcome.rs` module doc;
  `opencode-tool-outcome-trace.json` `source` field and cases.
- Findings: the OpenCode 1.19.0 trace shows an integer `exit` on `bash`
  parts and a null `exit` case.
- Missing evidence: re-run `callID` semantics, the other half of OQ3, belong
  to U7.
- Conclusion: resolved with answer - `state.metadata.exit` on `bash` only,
  landed in U5 with the pinned fixture.

### Q: Does the summarizer digest read this record?
- Sources examined: `tool_outcome::from_output`, `outcome_digest` call sites
  in `history_summarizer_chunk.rs` and `boundary.rs`.
- Findings: both digest sites read wire blocks through `from_output` and
  pass `exit_code: None`; `docs/compression-fidelity.md` states the same.
- Missing evidence: none.
- Conclusion: resolved with answer - the module is shared; the native
  extractors are not; reported to the lead.
