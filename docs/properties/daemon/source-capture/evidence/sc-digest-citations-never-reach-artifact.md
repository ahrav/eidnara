# sc-digest-citations-never-reach-artifact

## Discovery trigger

Part B obligation from #1066 comment 2, decision D9. The U6 ticket (#1088)
names the record `always(!X)` with faults F16 (budget truncation inside a
part) and F17 (a forged alias marker), and the acceptance clause that a
citation through a digest line resolves to digest bytes.

## Evidence trail

`crates/daemon/src/history_summarizer_chunk.rs` - `push_message` appends each
tool-result digest to the message's compacted text with
`tool_outcome::append_digests` before it builds the part. The part is
verbatim only when the message has one text block whose text equals the
compacted text, so a part carrying a digest is `transformed = true`.
`absorb_tool_only` builds tool-only parts with `transformed = true`.
`Builder::part` replaces `ALIAS_OPEN` (U+00AB) and `ALIAS_CLOSE`
(U+00BB) with `ALIAS_ESCAPE` across the whole part and marks a changed part
transformed; `tool_outcome::outcome_digest` already replaced the brackets in
the excerpts with `'`. `FrozenAliasTable::issue` names parts `s1`, `s2`, and
so on, in rendering order. `presented_input` truncates the joined text to the
token budget and keeps only the aliases whose marker followed by the whole
`presented` bytes occurs, in order, in the kept prefix; the first alias not
found whole and every later alias are withdrawn.

`crates/daemon/src/history_summarizer_citations.rs` - `FrozenAlias.presented`
holds the bytes shown to the model; validation accepts a citation
`[sN:start-end]` only when `frozen.presented.get(start..end)` is a span on
character boundaries. No code path maps a citation to native artifact
offsets. No production code reads `FrozenAlias.transformed` at HEAD; it is
serialized with the table and asserted by tests.

`crates/daemon/src/boundary.rs` - the boundary site renders digests for token
estimation (`ChunkEstimate.formatted_blocks`) and issues no aliases.

Landed checks:
`history_summarizer_chunk.rs::digest_chunk_tests::an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing`
puts `«s9»` in an error output and asserts the digest shows `'s9'`, the text
holds no `«s9»`, `s9` is not issued, and every marker in the text names an
issued alias.
`history_summarizer_chunk.rs::digest_chunk_tests::a_digest_cut_by_the_budget_withdraws_its_alias`
finds the largest budget whose cut keeps the digest's marker but not its
whole bytes, and asserts the digest alias is withdrawn, a strict prefix of it
remains, and every remaining alias's marker and bytes occur in the presented
text.
`history_summarizer_citations_golden.rs::rendered_aliases_match_the_hand_checked_native_oracles`
over `testdata/history_summarizer-citations-golden.json` pins a digest part
`TC: read(src/lib.rs); completed; bytes=9; head="file body"` as alias `s2`
with `transformed: true`.
`history_summarizer_citations_golden.rs::a_forged_marker_in_native_text_cannot_keep_a_withdrawn_alias`
and `truncation_withdraws_aliases_the_model_did_not_see_whole` cover the same
rules for native text.

## Failure scenario

A digest appended after `presented_input` runs, or a part marked verbatim,
lets a citation claim native bytes the model saw only as an excerpt. A
forged `«s9»` in tool output survives as a marker and matches an issued
alias during withdrawal, so a cut alias stays citable.

## Timing windows and dependencies

The window is the budget cut: `presented_input` runs after the full chunk is
built (`lib.rs` builds with `usize::MAX` and then calls `presented_input`
with the fired prompt's budget), so withdrawal sees every digest byte.

## What a test must construct

A tool output holding a marker literal, a chunk at a budget that cuts inside
a digest, and a citation validated against the digest alias. The forbidden
state has no detection point, so the check asserts on every built chunk that
each digest-bearing alias is `transformed` and each remaining alias's marker
and bytes occur whole. The legal witness is an error result cited through its
digest line; no landed test validates a citation into a digest alias.

## Investigation log

### Q: Does any reader treat a non-transformed alias as native offsets?
- Sources examined: repository-wide search for `.transformed`.
- Findings: only the struct definition, `Builder::part`, and the golden
  tests reference it.
- Missing evidence: none at HEAD.
- Conclusion: resolved with answer - no reader at HEAD; the guarantee rests
  on validation against `presented`.

### Q: Which configuration reaches this path?
- Sources examined: `DaemonConfig::default`, `eidnara_folds`, the summarizer
  fire path in `lib.rs`.
- Findings: aliases exist only on a summarizer fold, which needs a non-empty
  `model_chain`; the default is empty.
- Missing evidence: a trace of the fire gate under the default config.
- Conclusion: unresolved, needs that trace; recorded as
  `explicit-config-only`.
