# p2-deferred-source-units-publish-on-a-later-pass

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S4, touchpoint C8. Refinement R6 narrowed it.

## Evidence trail

The source-capture memo records `Disposition::Pending { since_ms }` (`crates/daemon/src/source_capture.rs:158`) for a unit that did not publish in its pass. The module documentation states that pending units are offered ahead of units the memo does not know. `crates/daemon/src/source_capture_tests.rs:428` publishes 32 units of a 33-unit window and the deferred unit on the next pass; `:464` publishes a unit pending behind a starting Kernel.

## Failure scenario

Without this state, U4 could not distinguish "the job ended" from "the descriptor exists".

## Timing windows and dependencies

The window exceeds one pass, or the Kernel is starting during the first pass.

## What a test must construct

A 33-unit window and a second pass; a Kernel still starting during the first pass.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/src/source_capture_tests.rs:180`, `:428`, `:464`, `:1072`, `:1233`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
