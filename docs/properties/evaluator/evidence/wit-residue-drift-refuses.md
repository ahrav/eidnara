# wit-residue-drift-refuses

## Discovery trigger
Parent specification: "Nondeterminism residue (recorded in the witness with
a drift gate modelled on the existing cross-root clock-column rule)";
redaction policy "refuse the frame; never substitute a placeholder".

## Evidence trail
- `crates/eval-core/src/witness.rs` `residue_drift`.
- `crates/eval-core/src/witness.rs` `serialize` encodes once, bounds the
  canonical bytes, and scans them; the canonical text is what the shell
  publishes.
- `crates/eval-core/src/cassette.rs` `scan_for_secrets` is shared with the
  cassette.
- `crates/daemon/examples/eval_runner/shrink.rs` `residue` unions the replay
  schema and the manifest schema; `Replayer::replay` refuses drift through
  `residue_drift` before the answer is kept.
- `crates/eval-core/tests/witness.rs` `residue_drift_refuses_and_limits_apply_before_publication`:
  drift, `TooLarge`, and a planted key refused.
- `crates/daemon/tests/eval_shrink.rs` `a_child_whose_residue_drifted_refuses_the_run`: a child that
  drops one entry refuses the run with one missing entry and no file
  published.
- `crates/daemon/tests/eval_shrink.rs` `a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`:
  `residue_drift` against a fresh child's report passes.

## Failure scenario
A later build reclassifies `pid` as `Keep`; without the gate the replay's
trace digest differs and the witness is reported not reproduced.

## Timing windows and dependencies
None.

## What a test must construct
A residue set with one entry removed; a package over a byte bound; a leaked
token in a string field; a child that reports a drifted set.

## Investigation log
### Q: Is drift a refusal or a recorded divergence?
- Sources examined: parent #749 wording; `Manifest::validate`.
- Findings: the manifest refuses residue contradictions; the witness follows
  the same rule.
- Missing evidence: none.
- Conclusion: resolved with answer - refusal.
