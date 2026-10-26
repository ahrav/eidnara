# uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero

## Discovery trigger

Spec #1145 C17 and lead L4.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip, the commit that carries this
revision.

- Ready gate: `packages/cli/src/commands/review-wire.ts:434`.
- Exact bound:
  `packages/opencode-plugin/src/shared/host-client/exact-json.ts:11`.
- Correction: the ready gate applies to the `memory_reviewer` block; no client
  decodes the `source_capture` block.

- Guarantee under test: `host.status` blocks carry closed-name counters, and the
  CLI renders the MemoryReviewer counters unavailable unless the block state is
  ready, through an exact-integer decoder bounded at 2^53.
- Existing check: `the_capture_report_reaches_host_status_field_for_field`
  (`crates/daemon/src/source_capture_tests.rs:845`);
  `packages/cli/src/commands/review.test.ts:887`;
  `packages/cli/src/commands/review.test.ts:938`;
  `packages/cli/src/commands/review.test.ts:849`. Status: unaudited.

## Failure scenario

A non-ready block with zero counters rendered as zero.

## Timing windows and dependencies

None.

## What a test must construct

Marker `uo_status_block_non_ready_with_counters_present`, owner #1157.

Required faults and enabling state: A `memory_reviewer` block in a non-ready
state with zero counters.

## Investigation log

### Q: Gap G6

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Gap G6: does the status surface name no identifiers? The
  `source_capture` and `memory_reviewer` blocks carry closed counter names only;
  owner #1157 (AC19) proves it for the use-outcome read.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
