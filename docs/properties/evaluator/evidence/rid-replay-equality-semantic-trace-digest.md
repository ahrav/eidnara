# rid-replay-equality-semantic-trace-digest

## Discovery trigger
Parent specification C-DST: "Replay equality is equality of a canonical
semantic trace digest over declared fields, never log bytes; the same identity
in two fresh OS processes yields equal manifest and trace digests."

## Evidence trail
- `crates/eval-core/src/residue.rs` `SemanticTrace::record` reduces each
  observation by its schema (`Keep`, `Drop`, `Presence`, `Relative`);
  `digest` is the protocol digest over the reduced entries.
- `crates/daemon/examples/eval_runner/shrink.rs:139` `replay_schema` keeps
  `scenario_digest` and `outcome`, drops `pid`.
- `crates/daemon/examples/eval_runner/shrink.rs:161` `child_main` records one
  observation and prints its digest with the outcome.
- `crates/eval-core/tests/two_process.rs:157` equality across two fresh processes; `:165` planted map-order
  leak fails it; `:195` a changed build component fails the identity check.
- `crates/daemon/tests/eval_shrink.rs:154` `replay_in_fresh_process`; `:175` two fresh replays of the
  minimized scenario are equal and the original replays to the recorded
  trace digest.

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
