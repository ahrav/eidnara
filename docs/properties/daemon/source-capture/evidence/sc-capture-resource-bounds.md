# sc-capture-resource-bounds

## Discovery trigger

Gap queued for U11 by the portfolio evaluation on #1066 (comment 2): "a
resource-boundary record (64 MiB payload cap, 1 KiB text field,
unbounded tool outputs)". Requirements R9, R10, R14 and decisions D3, D13
set the bounds; U1 (#1080), U2, U5, U6, and U10 landed them. This
record collects them in one place and was authored from HEAD.

## Evidence trail

Kernel, applies to every artifact ingest (`crates/kernel/src/cas/`):

- `MAX_PAYLOAD_BYTES = 64 * 1024 * 1024` (`cas/mod.rs`);
  `PreparedArtifact::new` (`cas/ingest.rs`) refuses one byte more as
  `PayloadTooLarge`.
- `MAX_TEXT_FIELD_BYTES = 1024` (`cas/mod.rs`); ingest refuses any text
  field (media type, retention class, producer, operation key, actor,
  cause, provenance) above it as `TextFieldTooLong`.
- `MAX_DESCRIPTORS_PER_COMMIT = 1024` (`source_descriptor.rs`).

Publisher (`crates/daemon/src/harness_sources.rs`):

- `MAX_SOURCE_UNIT_BYTES` = `memory_store::memory_capture::MAX_CAPTURE_SOURCE_BYTES`
  (64 KiB). `publish_screened` checks `unit.text.len()` first and gives
  `UnitDisposition::UnitTooLarge` before `prepare`, so nothing is
  retained and the unit takes no batch slot.
- `MAX_SOURCE_BATCH_UNITS` = `memory_capture::MAX_CAPTURE_MESSAGES` (32)
  and `MAX_SOURCE_BATCH_BYTES` = `memory_capture::MAX_CAPTURE_INPUT_BYTES`
  (64 KiB). The first unit that would exceed either bound and every later
  unit within the size cap are `Deferred`.
- The 512-byte identity cap refuses a unit before retention (Part A
  `sc-adapter-identity-native-or-refused`).
- Receipt keys are digests (`receipt_key`), so capture's ingest text
  fields stay far below 1 KiB.

Outcome record (`crates/kernel/src/source_descriptor.rs`):
`ToolOutcome::validate` requires `encode().len() <= MAX_TEXT_FIELD_BYTES`;
`check_outcome` runs in `SourcePublisher::prepare` before retention and
again in the descriptor commit. `tool_outcome.rs` caps
`MAX_EXCEPTION_NAME_BYTES` at 64.

Summarizer digest (`crates/daemon/src/tool_outcome.rs`):
`MAX_DIGEST_BYTES = 1024`; each excerpt is clipped to
`CLAIM_VALUE_MAX_BYTES` on a character boundary with `…`.

Supervisor (`crates/daemon/src/source_capture.rs`):

- `MAX_RETAINED_WINDOW_BYTES = 32 MiB`. `SourceCapture::offer` charges
  `(window + document text) * 2 + ARTIFACT_PREPARATION_BYTES`
  (`4 * MAX_SOURCE_BATCH_BYTES`); above the bound is
  `refused_window_too_large`, short of the remaining budget is
  `refused_resident_short`. `CaptureJob::publish` tops up for adapter
  copies or ends `refused_resident_short`.
- `MEMO_BUDGET_BYTES = 4 MiB`; `RETAINED_BYTES_BOUND` is the sum and is
  declared in the daemon's retained-bytes total (`lib.rs`).
- One publication slot per store (`into_job`, `refused_queue_full`) and
  `CAPTURE_BUDGET = 5 s`.

Tool output path: `opencode_units` copies the whole `state.output` or
`state.error` string into one unit; `pi_units` copies each text item. The
adapters do not truncate. An output above 64 KiB is refused whole by the
publisher screen; it is never cut to a prefix.

Documents: `MAX_PROJECT_DOC_BYTES = 256 KiB` (`project_docs.rs`) bounds
the read; `plan_document` applies `MAX_SOURCE_UNIT_BYTES` to the text and
counts both as `documents_oversize`.

## Failure scenario

A tool output is truncated to a prefix and retained as if exact; or the
Kernel's 64 MiB cap is the only limit, so one unit holds tens of MiB; or
an oversized unit fails its whole batch; or resident memory grows with
window size; or an outcome record or digest grows with tool output.

## Timing windows and dependencies

- Window-level refusal precedes the unit screen. A single tool output
  larger than about 16 MiB makes the doubled window charge exceed 32 MiB,
  so the whole window is `refused_window_too_large` and its siblings are
  not captured on that pass. The next pass that carries the same record
  is refused the same way. The per-unit "siblings still publish" rule
  holds only for windows under the retained bound.
- A unit at the cap publishes; one byte over is terminal for unchanged
  input (`classify` records `Terminal`).

## What a test must construct

A unit of exactly 65 536 bytes and one of 65 537 bytes beside siblings; a
33-unit window; three units of a third of 64 KiB plus one; an oversized
outcome record; arbitrary multibyte outputs through the digest; a window
above the retained bound and a second window past the remaining budget;
100 000 memo entries; a 70 KiB document. Missing: a tool unit above
64 KiB through the capture path, and a window holding a very large tool
output beside small siblings.

## Investigation log

### Q: Is a tool output above 64 KiB exercised through the capture path?
- Sources examined: `source_capture_tests.rs` (no `units_unit_too_large`
  assertion); `harness_sources.rs` tests using `pi_text_unit`.
- Findings: the publisher tests use Pi message units. The screen is
  class-agnostic (`unit.text.len()`), so tool units take the same branch,
  but no landed test constructs one.
- Missing evidence: a test with an oversized `raw_tool_spans` unit.
- Conclusion: unresolved, needs a tool-output test at 64 KiB + 1 through
  `CaptureJob`.

### Q: Should a very large tool output refuse its whole window?
- Sources examined: `SourceCapture::offer`; R10 wording.
- Findings: as described under timing windows.
- Missing evidence: an owner decision whether R10's sibling rule must
  hold at the window charge.
- Conclusion: needs human input.

### Q: What bounds the native window before it reaches capture?
- Sources examined: not read.
- Findings: the host wire frame limit bounds the transform request; this
  agent did not locate it.
- Missing evidence: the wire frame cap at HEAD.
- Conclusion: unresolved, needs the host wire frame bound.
