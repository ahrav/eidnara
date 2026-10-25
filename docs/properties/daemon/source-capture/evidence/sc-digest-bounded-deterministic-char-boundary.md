# sc-digest-bounded-deterministic-char-boundary

## Discovery trigger

Part B obligation from #1066 comment 2, requirement R9 and decision D9. The
U6 ticket (#1088) names the record with CRLF, multibyte, and single-line
inputs, requires byte-equal digests for equal inputs, and resolves OQ17 by
rendering both `TC:` sites through one formatter.

## Evidence trail

`crates/daemon/src/tool_outcome.rs` - `outcome_digest(status, exit_code,
output)` writes `; <status>`, `; exit=N` when an exit code is given, and
`; bytes=<output.len()>`. It takes the nonblank lines of `output` with
`str::lines` (which drops a trailing `\r` before `\n`) and `trim_end`, uses
the first as `head` and the last as `tail` (a single nonblank line is `head`
alone), replaces `"`, U+00AB, and U+00BB with `'`, and clips each excerpt
with `clip(.., CLAIM_VALUE_MAX_BYTES)`. `clip` keeps text that fits;
otherwise it backs the cut off to a `is_char_boundary` position that leaves
room for the 3-byte `…` marker and appends it, so a clipped excerpt is at
most 128 bytes. `digest(call, outcome)` clips the call summary to
`MAX_DIGEST_BYTES - outcome.len()` and appends the outcome half, so the
whole digest is at most `MAX_DIGEST_BYTES` (1024). The functions read no
clock, map, or global state, so equal inputs give equal bytes.
`from_output` maps each `memory_store::OutputKind` to a status and text.

Both sites pass `exit_code: None`: `history_summarizer_chunk.rs::tool_parts`
and `boundary.rs::tool_results`. The digest therefore shows no `exit=` field
on either site; `docs/compression-fidelity.md` ("Tool outcome digest")
states this.

`crates/daemon/src/history_summarizer_citations.rs` -
`CLAIM_VALUE_MAX_BYTES` is the 128-byte claim-value bound the excerpts reuse.

Landed checks (all in `crates/daemon/src`):
`tool_outcome.rs::digest_tests::a_digest_shows_status_bytes_and_the_outer_nonblank_lines`
pins CRLF output, a single-line output, an empty output, and an error with
exit 2; `a_digest_clips_on_character_boundaries_within_its_bounds` pins
two-byte and three-byte clipping and a call summary clipped to exactly 1024
bytes; `a_wire_output_maps_its_variant_to_the_status` and
`a_status_spells_as_the_record_stores_it` pin the status mapping and
spelling; the proptest
`every_digest_is_bounded_and_shows_only_its_outer_lines` generates lines of
multibyte characters, quotes, marker brackets, tabs, and `\r`, with LF or
CRLF, and a call summary up to 1500 characters, and asserts the 1 KiB bound,
the 128-byte excerpt bound, the excerpt equal to `clip` of the escaped outer
line, and a middle-line sentinel absent.
`history_summarizer_chunk.rs::digest_chunk_tests::the_c5_case_digest_shows_completed_and_the_refusal_in_tail`
decodes the C5.V1 corpus source and asserts the `completed` digest with the
`connection refused` line in `tail`.
`history_summarizer_chunk.rs::digest_chunk_tests::tool_output_contains_sentinel`
and `boundary.rs::digest_tests::the_boundary_chunk_renders_digests_for_answered_calls`
assert a middle-line sentinel never reaches the chunk at either site.

## Failure scenario

A byte-indexed `&s[..128]` on a multibyte line panics or splits a code point
in the prompt. A digest built from the full output sends the whole log to the
summarizer.

## Timing windows and dependencies

None. The formatter is a pure function of the wire block and the call
summary.

## What a test must construct

CRLF, multibyte, single-line, and empty outputs; an output whose outer lines
exceed 128 bytes; a call summary longer than 1 KiB; a sentinel on a middle
line at both construction sites; and two builds over equal inputs compared
byte for byte. The legal witness is the C5 case showing `completed` and the
refusal text in `tail`. The coverage witness `tool_output_contains_sentinel`
marks the sentinel precondition.

## Investigation log

### Q: Does the boundary site render the same digest (OQ17)?
- Sources examined: `ticket-1088.md`; `boundary.rs::tool_call_parts` and
  `tool_results`; the boundary test.
- Findings: the boundary site calls the same `outcome_digest` and `digest`
  with `format_tool_summary`.
- Missing evidence: none.
- Conclusion: resolved with answer - one formatter at both sites, landed in
  U6.

### Q: Which configuration reaches the presented digest?
- Sources examined: `config.rs` (`DaemonConfig::default` sets an empty
  `model_chain`; `eidnara_folds` requires it non-empty); the summarizer fire
  path in `lib.rs` that calls `build_history_summarizer_chunk` and
  `presented_input`; the boundary trigger path through
  `chunked_message_estimate_with_estimator`.
- Findings: the source record labelled the path `default-production`; the
  presented digest needs a summarizer fold, which needs a configured model.
- Missing evidence: a trace showing whether the boundary trigger estimate
  runs for a session whose fold authority is the host's native compaction.
- Conclusion: unresolved, needs a read of the trigger gating under
  `DaemonConfig::default()`; recorded as `explicit-config-only`.
