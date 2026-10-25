# sc-consented-window-reaches-publisher

Abbreviations follow [`../catalog.md`](../catalog.md): `hs.rs`, `hs-tests`,
`sc.rs`, and `sc-tests` name the harness-source and source-capture modules
and their tests.

## Discovery trigger

The fresh-context portfolio evaluation (finding E2 in
[`../portfolio-evaluation.md`](../portfolio-evaluation.md)) found that no
record carried R1's reach obligation once
`sc-harness-classes-have-no-production-publisher` was invalidated: every
OpenCode or Pi message text block, tool output, and tool error in a
transform window reaches `SourcePublisher` in production. The invalidated
record's successor, `sc-consent-gate-admits-capture`, bounds when
publication may happen; this record states that it does happen.

## Evidence trail

- `crates/daemon/src/lib.rs` `Handler::handle_transform_typed` builds a
  `source_capture::Offer` when `source_capture::consents` holds, and
  `Handler::capture_after_response` hands it to `SourceCapture::offer`, which
  charges the window and returns a `CaptureHandoff`.
- `sc.rs` `CaptureHandoff::into_job` takes the slot after the response
  settles; `CaptureJob::publish` runs `CaptureJob::units`, which calls
  `opencode_units` or `pi_units` for every record, and
  `CaptureJob::publish_locked` publishes the screened batch through
  `hs.rs` `publish_screened`.
- `CaptureJob::capture_documents` publishes the documents the pass read at
  its m0 compose read through a second `publish_screened` batch.
- `sc-tests`
  `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
  sends a user text block, a failed `bash` tool, and a completed `read` tool
  and reads the `messages`, `tool_error`, and `tool_output` descriptors with
  their exact bytes.
- `sc-tests` `a_pi_window_publishes_user_text_and_tool_results` sends Pi rows
  carrying `type` and `parentId`, one tool result with `isError: false` and
  one with `isError: true`, and reads the user text, a `tool_output` row,
  and a `tool_error` row.
- `sc-tests`
  `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`
  reads the `repository_documents` descriptor a composing pass publishes.

## Failure scenario

A change to the handoff or the job drops one kind of unit, for example tool
errors, before `publish_screened`. The response stays byte-identical, every
safety record still holds because nothing is published, and the retained
history silently lacks the bytes R1 promises.

## Timing windows and dependencies

The units reach the publisher after the response settles. A test reads the
store only after `settle`, which waits for the capture job to finish. The
reach is bounded by the batch bounds: a unit past 32 units or 64 KiB waits
for the next pass (`sc-pending-unit-publishes-next-ready-pass`).

## What a test must construct

A consented handler with a real Kernel store, one pass per harness whose
window holds a user text block, a tool output, and a tool error, and a
composing pass with a configured document; then a read of the live
descriptors by class and representation.

## Investigation log

### Q: Does every kind of unit have a reaching test?
- Sources examined: the three tests above and `CaptureJob::units`.
- Findings: OpenCode text, `tool_output`, and `tool_error`; Pi text,
  `tool_output`, and `tool_error`; and repository documents each reach a
  live descriptor in a test.
- Missing evidence: none for the kinds the check names.
- Conclusion: resolved with answer.

### Q: Is `reachable` the right semantics?
- Sources examined: `../../../METHOD.md` check semantics.
- Findings: the obligation is that a production path executes for each kind
  of unit, which is location coverage over the publisher call.
- Conclusion: resolved with answer: `reachable`.
