# p2-reachable-from-holds-on-ancestor-of-head

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S1, touchpoints C3, N1, and AC3. The U3 stop condition ("stop if the anchor kind makes every memory Historical after one commit") depends on it.

## Evidence trail

`crates/kernel/src/applicability/resolve.rs:179` (`ResolutionLadder::evaluate`) maps `ReachableFrom` through `resolve_commit` (`:230`). That function parses HEAD and the anchor oid, checks the anchor commit is present, and calls `is_ancestor_or_equal_oid` (`:527`); `Some(true)` returns `Reachable` before any fallback rung runs. The engine maps `Holds` to `Current`.

`crates/kernel/tests/kernel_anchor_resolution.rs:44` builds a history where HEAD is a descendant of the anchor and a foreign branch is not, and asserts both verdicts. `:159` covers criss-cross merges and `:557` a rebase merged through a second parent. `crates/kernel/tests/kernel_applicability_engine.rs:806` checks an object with no anchor claims no held anchor.

## Failure scenario

An ancestry test that compared HEAD for equality instead of ancestry would turn every captured memory `Historical` after the next commit.

## Timing windows and dependencies

None within one evaluation. Across evaluations, the HEAD the snapshot read is the HEAD the verdict describes.

## What a test must construct

A fixture repository with commit X, a descendant Y at HEAD, and a foreign branch Z. Evaluate `ReachableFrom { oid: X }` at HEAD = X, HEAD = Y, and HEAD = Z.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_anchor_resolution.rs:44`, `:159`, `:557`; `crates/kernel/tests/kernel_applicability_engine.rs:806`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
