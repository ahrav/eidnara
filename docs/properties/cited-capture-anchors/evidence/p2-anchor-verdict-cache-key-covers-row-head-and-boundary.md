# p2-anchor-verdict-cache-key-covers-row-head-and-boundary

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S1, touchpoint N1. Portfolio refinement R4 tightened its check.

## Evidence trail

`AnchorCacheKey` (`crates/kernel/src/applicability/engine.rs:205`) holds `checkout_identity`, `row_fingerprint`, `head`, `repository_state`, and `patch_id_algorithm`. The batch memo and the cross-batch cache both key on it. The fourteen cited engine tests move HEAD (`crates/kernel/tests/kernel_applicability_engine.rs:206`), change the shallow boundary (`:2621`, `:3943`) and sparse patterns (`:2659`), share an anchor id across rows (`:1064`, `:2881`), distinguish an empty payload from an absent one (`:1029`), and refuse to cache verdicts tainted by absent objects (`:3080`, `:3434`, `:3676`, `:4095`).

## Failure scenario

A cached `Holds` reused after a reset dropped the anchored commit would keep a historical memory current.

## Timing windows and dependencies

HEAD or the shallow boundary moves between two batches against one cache.

## What a test must construct

Pairs of evaluations differing in exactly one key component, asserting the second resolves afresh; plus two rows with one `anchor_id` and different payloads.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: the fourteen `crates/kernel/tests/kernel_applicability_engine.rs` tests named in the record, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does "differ in exactly one component" change what the tests prove?

- Sources examined: the cited tests and refinement R4.
- Findings: Each cited test changes one input; the tightened wording states that, so a test changing two components no longer counts as evidence for either.
- Missing evidence: none.
- Conclusion: resolved with answer.
