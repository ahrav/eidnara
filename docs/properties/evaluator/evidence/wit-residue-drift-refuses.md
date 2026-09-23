# wit-residue-drift-refuses

## Discovery trigger
Parent specification: "Nondeterminism residue (recorded in the witness with
a drift gate modelled on the existing cross-root clock-column rule)";
redaction policy "refuse the frame; never substitute a placeholder".

## Evidence trail
- `crates/eval-core/src/witness.rs:196` `check_residue`.
- `crates/eval-core/src/witness.rs:209` `serialize` bounds bytes and scans.
- `crates/eval-core/src/cassette.rs` `scan_for_secrets` shared with the
  cassette.
- `crates/daemon/examples/eval_runner/shrink.rs:151` `residue` unions the
  replay schema and the manifest schema; `:266` drift refuses the run.
- `crates/eval-core/tests/witness.rs:260` drift, `TooLarge`, and a planted key refused.
- `crates/daemon/tests/eval_shrink.rs:175` `check_residue` against a fresh child's report passes.

## Failure scenario
A later build reclassifies `pid` as `Keep`; without the gate the replay's
trace digest differs and the witness is reported not reproduced.

## Timing windows and dependencies
None.

## What a test must construct
A residue set with one entry removed; a package over a byte bound; a leaked
token in a string field.

## Investigation log
### Q: Is drift a refusal or a recorded divergence?
- Sources examined: parent #749 wording; `Manifest::validate`.
- Findings: the manifest refuses residue contradictions; the witness follows
  the same rule.
- Missing evidence: none.
- Conclusion: resolved with answer - refusal.
