# sc-reduction-drops-body-keeps-failure-variant

## Discovery trigger

Part A record of the source-capture catalog (#1066 comment 1), written at
baseline `08de9051a` for AE2: once reduction drops a tool arc, the result's
body survives only in a retained source. U6 lists this record as exercised and
unchanged (`ticket-1088.md`). This file re-verifies it at HEAD
`5a842d998`.

## Evidence trail

`crates/daemon/src/selection.rs`:

- `DROPPED_PLACEHOLDER` is the constant `"[dropped]"`.
- `expand_arc` emits, for `ArcShape::FullDrop` and `ArcShape::DedupFullDrop`,
  a `ReductionDecision` of kind `RedKind::Drop` with payload
  `DROPPED_PLACEHOLDER` for every call id and every result id the `frozen`
  set does not hold. Result ids always take `Drop`, whatever the shape.
- `select_reductions_with_outcome` calls `expand_arc` after the resolver.

`crates/daemon/src/transform.rs`:

- `apply_once` calls `select_reductions_with_outcome` under `producer_gate`
  on every transform pass, with no configuration gate beyond the pass itself.
- The rebuild loop in `apply_once` calls `wire::reduced_block` with the frozen
  unit's payload. When the unit's payload is `"[dropped]"` and a tag overlay
  names the block, the display payload is `"[dropped §N§]"` instead.

`crates/daemon/src/wire.rs`:

- `reduced_block` maps `OutputKind::{Text, Json, Content}` to `Text`,
  `OutputKind::{ErrorText, ErrorJson, ErrorContent}` to `ErrorText`, and
  `ExecutionDenied` to `ExecutionDenied { reason: Some(reduced) }`. It clones
  the result `id`, `tool_name`, `provider_executed`, the output's
  `provider_extras`, and the block's provider extras.

`crates/daemon/src/codec/opencode.rs`: the OpenCode decoder builds the result
block from `state.output` or `state.error` and marks the error variant when the
tool status is `error` (the `tool_output_from_part` call in the tool-part
decoder). This is the source of the `Error*` variants reduction preserves.

Tests read at HEAD:

- `wire.rs::reduced_tool_result_keeps_failure_variant_and_output_extras`
  constructs all eight `OutputKind` cases and asserts the mapped variant, the
  kept identity, and both extras maps.
- `selection.rs::frozen_arc_blocks_never_re_emitted` freezes one result id and
  asserts no decision names any block of that arc.
- `transform.rs::agent_drop_ids_freeze_add_only_through_flat_ids`,
  `coverage_extending_soft_prunes_covered_red_units`,
  `host_reversibility_true_reduction_stays_canonical_after_false_flip`, and
  `owned_profile_drains_pending_drops_without_request_surface` assert that the
  served tail bytes of a dropped block are `"[dropped]"`.
- `transform.rs::reduced_block_renders_placeholder_without_tag_prefix`
  asserts the served bytes `"[dropped §2§]"` under the tag overlay.

## Failure scenario

A reduction that collapses `ErrorText` to `Text` tells the model a failed call
succeeded. A reduction that keeps any native output byte in the reduced block
leaks the body the drop was meant to remove. A frozen result re-emitted with a
new payload changes a cached prefix.

## Timing windows and dependencies

None in the reduction itself; it is a pure mapping per block. The frozen set
comes from `frozen_red_targets(&loaded.core)` read earlier in `apply_once`.

## What a test must construct

A tool arc selected for `FullDrop` or `DedupFullDrop` (fault F20) whose result
block carries each of the eight `OutputKind` variants with a distinctive body;
one arc whose result id is frozen; a tag overlay that numbers the dropped
block. Assert per decision, per reduced block, and by body-byte absence.

## Investigation log

### Q: Do the baseline citations still hold?

- Sources examined: `selection.rs`, `wire.rs`, `codec/opencode.rs`,
  `transform.rs` at HEAD; the same files at `08de9051a`.
- Findings: `wire.rs:1199` is still the start of
  `reduced_tool_result_keeps_failure_variant_and_output_extras`. The
  transform tests moved: baseline `:17077`, `:22357`, `:24934`, `:25748` are
  now inside the four test functions named above. The baseline
  `selection.rs:529` is now `DROPPED_PLACEHOLDER` at `:531` and the
  `expand_arc` range is `:661-694`. `codec/opencode.rs:513-522` still points at
  the tool-output decode.
- Missing evidence: none.
- Conclusion: resolved with answer; citations are now by name.

### Q: Is the placeholder always the literal `[dropped]`?

- Sources examined: `apply_once` rebuild loop;
  `reduced_block_renders_placeholder_without_tag_prefix`.
- Findings: the decision payload is always `"[dropped]"`. The served block text
  is `"[dropped §N§]"` when a tag overlay names the block and the unit's reset
  rule is not `RED_SUPPRESS_TAG_OVERLAY_RULE`. The test name says "without tag
  prefix" while it asserts the tagged form.
- Missing evidence: none.
- Conclusion: resolved with answer; the record's check states both forms.

### Q: Can provider extras carry native output bytes into the reduced block?

- Sources examined: `reduced_block`; the OpenCode decoder.
- Findings: `reduced_block` copies both extras maps verbatim. No test places a
  body byte inside an extras value.
- Missing evidence: a decoder audit of which native fields land in extras.
- Conclusion: unresolved, needs an audit of extras population per codec.
