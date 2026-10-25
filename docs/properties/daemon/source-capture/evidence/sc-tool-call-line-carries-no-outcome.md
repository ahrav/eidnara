# sc-tool-call-line-carries-no-outcome

## Discovery trigger

Part A record of the source-capture catalog (#1066 comment 1), written at
baseline `08de9051a`: the summarizer chunk rendered a tool call as a bare
`TC:` line with no outcome, which hid #707 C5 (a `completed` tool whose output
reports failure). U6 (`ticket-1088.md`) replaced the bare line with a bounded
outcome digest, resolved OQ17 by rendering both `TC:` sites through one
formatter, and added a sentinel-absence test. This file re-verifies the
record at HEAD `5a842d998`.

## Evidence trail

`crates/daemon/src/history_summarizer_chunk.rs`:

- `format_tool_summary` returns `TC: {description}`, else
  `TC: {name}({key_arg})`, else `TC: {name}`.
- `tool_parts` renders each tool result as
  `tool_outcome::digest(call, outcome)`, where `call` is the call's summary in
  the same message, else the summary `build_tool_call_summary_lookup` holds for
  the arc, else `TC: {tool_name}`. With `bare_calls`, an unanswered call renders
  its summary alone.
- `ChunkBuilder::part` escapes U+00AB and U+00BB to `"` and marks the part
  `transformed` when it escaped anything; every tool-only part is built with
  `transformed: true`, and a narrative part is `transformed` unless it is one
  text block presented verbatim.
- `presented_input` withdraws every alias whose marker and presented bytes do
  not both survive in the kept prefix.

`crates/daemon/src/tool_outcome.rs`:

- `from_output` maps the wire `OutputKind` to `ToolStatus::Completed` or
  `ToolStatus::Error` plus the output text.
- `outcome_digest` emits `; <status>[; exit=N]; bytes=N[; head="…"][; tail="…"]`.
  Output bytes enter only through `head` and `tail`, the first and last nonblank
  lines, clipped to `CLAIM_VALUE_MAX_BYTES` on a character boundary.
- `digest` clips the call summary so the whole digest is at most
  `MAX_DIGEST_BYTES` (1024).

`crates/daemon/src/chunk_text.rs::extract_key_arg`: the first of
`filePath|path|pattern|query`, cut at 60 scalars with `…`, else the first of
`symbol|module|action` uncut.

`crates/daemon/src/boundary.rs`: `tool_results` builds an outcome digest per
arc from `from_output` and `outcome_digest`; `tool_call_parts` renders
`tool_outcome::digest(&format_tool_summary(..), outcome)` for an answered call
and the bare summary otherwise; `chunk_message_part` appends digests through
`tool_outcome::append_digests`.

Both production call sites pass `exit_code: None`, so the summarizer view never
shows `exit=` at HEAD.

Reachability: `build_history_summarizer_chunk` is called from
`maybe_spawn_reattach` in `crates/daemon/src/lib.rs` and from the firing path in
`transform.rs`; the boundary chunk runs inside
`check_history_segment_trigger_with_index`. Neither path reads
`source_capture.enabled`. The summarizer's own firing conditions are not
re-verified here.

## Failure scenario

A tool output byte outside the two excerpts reaches the summarizer prompt, so
the full log or a secret in its middle is summarized. A forged marker in output
becomes a citation target. A budget cut inside a digest leaves its alias live,
so a citation names bytes the model never saw whole.

## Timing windows and dependencies

None at runtime; the chunk builder is a pure function of admitted blocks and a
token budget. Truncation (F16) happens after rendering, in `presented_input`.

## What a test must construct

Tool results whose middle lines carry a sentinel, at both sites
(`tool_output_contains_sentinel` precondition); an error result; a forged
`«sN»` marker in output (F17); a budget that cuts inside a digest (F16); an
unanswered call in a message with and without narrative.

## Investigation log

### Q: OQ17. Does the digest apply at the second `TC:` site in `boundary.rs`?

- Sources examined: `ticket-1088.md` resolution; `pr-1088.md`; `boundary.rs`
  `tool_results`, `tool_call_parts`, `chunk_message_part`; test
  `boundary.rs::the_boundary_chunk_renders_digests_for_answered_calls`.
- Findings: both sites call `format_tool_summary`, `outcome_digest`, `digest`,
  and `append_digests`. The boundary test asserts the digest for an answered
  call, the bare line for an unanswered one without narrative, the absence of
  the unanswered call when narrative exists, and the absence of
  `SENTINEL-7f3a`.
- Missing evidence: none.
- Conclusion: resolved with answer in U6.

### Q: Does the record's name still match the behavior?

- Sources examined: the U6 test doc comment on
  `history_summarizer_chunk.rs::tool_output_contains_sentinel`.
- Findings: the line now carries status, byte count, and two excerpts. The
  baseline guarantee "no status, exit code, or error text" is replaced by "no
  output byte outside the two excerpts". The test comment says the record holds
  "as the digest restates it".
- Missing evidence: a lead decision on renaming or invalidating the slug.
- Conclusion: needs human input (lead disposition); the record carries the
  restated guarantee.
