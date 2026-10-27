# uo-subagent-pass-serves-no-composition-under-eidnara-folds

## Discovery trigger

Spec #1145 K12 and lead L1.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip, the commit that carries this
revision.

- Folding plan rule: `crates/daemon/src/transform.rs:4239` sets `Defer` when the
  scheduler defers and `Soft` otherwise.
- Subagent stepping: `crates/daemon/src/transform.rs:4478`.
- Routing: `crates/daemon/src/transform.rs:3363`.
- Correction: the spec cited `transform.rs` line 27022, now inside the test
  `terse_text_compression_gates_subagents_reasoning_and_protected_tail`.

- Guarantee under test: Under `eidnara_folds`, a subagent pass plans `Soft` or
  `Defer`, enters no HARD arm, and is served no new composition; under native
  authority the additive engine applies no subagent plan rule.
- Existing check:
  `a_subagent_pass_under_native_authority_reports_native_authority`
  (`crates/daemon/src/fold_authority_handler_tests.rs:143`);
  `subagent_first_active_render_commits_tagged_bytes_before_replay`
  (`crates/daemon/src/transform.rs:23784`);
  `the_session_role_follows_the_pass_subagent_flag_on_each_engine`
  (`crates/daemon/src/use_outcome_writer_tests.rs:412`). Status: unaudited.

## Failure scenario

A subagent pass under `eidnara_folds` that entered a HARD arm would freeze and
serve a composition the folding contract says it never receives.

## Timing windows and dependencies

None.

## What a test must construct

A subagent request on a route with `eidnara_folds`, asserting no
`<project-memory>` block and no composition; the #1155 writer test then asserts
no row.

Required faults and enabling state: A route with `eidnara_folds` (compaction on,
a model chain) and a subagent request; the same request under native authority.

## Investigation log

### Q: OQ3-13

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-13: is the additive engine's subagent serve intended? Owner:
  product owner with #1142 (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: Correction

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Correction: a native subagent lineage switch routes to `apply_once`
  and answers a passthrough (`crates/daemon/src/transform.rs:3363`), so "no
  subagent rule applies under native authority" holds for the plan, not for
  routing.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
