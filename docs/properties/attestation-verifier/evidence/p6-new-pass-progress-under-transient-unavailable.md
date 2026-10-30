# p6-new-pass-progress-under-transient-unavailable

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. Converted to active by [#1173](https://github.com/ahrav/eidnara/issues/1173), which lands the bounded pass in `crates/daemon/src/attestation_verifier/bounded_pass.rs`.

## Evidence trail

A verdict of a transient reason (`deadline`, `object_unreadable`, `repository_unopenable`, `repository_unbound`), a Kernel refusal of the classification or the commit, a `changed` decision, and a degraded witness each call `retry` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:464`), which counts one attempt per pass for the decision; the K-th (`MAX_ATTEMPTS`, `:62`, 3) resolves it under `unavailable_skipped`, or `witness_degraded_skipped` for a degraded witness, so the cursor passes it. A pass continues past an unresolved decision to the ones behind it within its budgets, and the task passes again after `RETRY_INTERVAL` (5 s) when a pass progressed or failed.

`a_transient_head_resolves_after_k_passes_and_a_terminal_reason_is_read_once` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:729`) places an unbound head before a `path_absent` decision and a verifiable one: the first pass counts `unavailable_repository_unbound = 1`, reads the `path_absent` decision once, and attests the verifiable one while the cursor stays before the head; the third pass counts `unavailable_skipped = 1` and moves the cursor past all three, and the next pass reports neither progress nor failure. `a_witness_degraded_k_passes_is_skipped_and_the_pass_continues` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:892`) degrades one decision's witness every pass: after three passes `witness_degraded = 3`, `witness_degraded_skipped = 1`, and a decision added behind it is attested.

## Failure scenario

A head decision consuming every pass.

## Timing windows and dependencies

Passes at the retry interval while the head's reason persists.

## What a test must construct

F13: a permanently transient head decision, as precondition coverage for `sometimes(head_decision_transient_unavailable)`.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1173 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does a repeated refusal or degradation stop the task?

- Sources examined: OQ6-11 in [#1173](https://github.com/ahrav/eidnara/issues/1173); `ProjectRun::outcome` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:565`).
- Findings: The owner confirmed the K bound for a degraded witness, then `witness_degraded_skipped`, and the task continues. The pass applies the same bound to Kernel refusals and `changed` outcomes, counting `unavailable_skipped` at the K-th.
- Missing evidence: none.
- Conclusion: resolved with answer.
