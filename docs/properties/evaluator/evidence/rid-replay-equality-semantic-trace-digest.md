# rid-replay-equality-semantic-trace-digest

## Discovery trigger
Parent specification C-DST: "Replay equality is equality of a canonical
semantic trace digest over declared fields, never log bytes; the same identity
in two fresh OS processes yields equal manifest and trace digests."

## Evidence trail
- `crates/eval-core/src/residue.rs` `SemanticTrace::record` reduces each
  observation by its schema (`Keep`, `Drop`, `Presence`, `Relative`);
  `digest` is the protocol digest over the reduced entries.
- `crates/daemon/examples/eval_runner/shrink.rs` `replay_schema` keeps
  `scenario_digest` and `outcome`, drops `pid`; `child_main` records one
  observation and prints its digest with the outcome and the residue.
- `crates/eval-core/tests/two_process.rs` `two_process_same_identity_yields_equal_manifest_and_trace_digests`
  is the equality across two fresh processes;
  `two_process_planted_map_order_leak_fails_the_equality_test` is the
  negative control; `two_process_changed_build_component_fails_the_identity_check`
  shows one changed identity component changes the run id.
- `crates/daemon/tests/eval_shrink.rs` `replay_in_fresh_process` spawns the child entrypoint; the
  published test asserts two fresh replays of the minimized scenario are
  equal and the original replays to the recorded trace digest.

## Failure scenario
A witness records a trace digest computed over a `HashMap` iteration; the
replaying process orders it differently and refuses a faithful replay as
drifted.

## Timing windows and dependencies
None beyond process start.

## What a test must construct
Two fresh processes over one input, comparing the printed digests, plus a
negative control with an order leak.

## Investigation log
### Q: One digest or one per stage?
- Sources examined: `SemanticTrace`; the shell schema.
- Findings: one digest over the ordered entries; the shrink replay has one
  observation.
- Missing evidence: none for this phase.
- Conclusion: resolved with answer - one digest per replay.
### Q: Does the shrink replay's trace carry anything the outcome does not?
- Sources examined: `replay_schema`.
- Findings: no; the kept fields are the scenario digest and the outcome, so
  equal outcomes imply equal digests there. The manifest two-process test
  carries the discriminating fields.
- Missing evidence: none.
- Conclusion: resolved with answer - cite both.
