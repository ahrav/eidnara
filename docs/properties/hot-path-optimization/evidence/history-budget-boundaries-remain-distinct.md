# history-budget-boundaries-remain-distinct

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

One budget abstraction can silently conflate the rendered body, wrapped history,
request validation, and replay. These have different implemented boundaries.

## Evidence trail

- [decay_render.rs:289-338][inner] guards only positive direct-API budgets.
  [203-240][tiers] uses tiers 1 through 5 and emits nothing at tier 5.
- [tokenizer/lib.rs:123-149][tokenizer] counts encoded tokens and defines zero
  tokens for empty input. At representable `H` and `5H`, at most `4H` demotions
  reach all-tier-5 output for `H` compartments, without requiring monotonic cost.
- [m0_compose.rs:178-215][outer] counts the wrapped history slice, retries above
  105% at most three times, and can return a still-over-budget render.
- [daemon/lib.rs:8239-8242][validation] resolves request budgets separately.
- [transform.rs:4081-4108][hard] composes on HARD; [4348-4381][refold] also
  composes on pressure refold. Frozen replay is not all SOFT work indiscriminately.
- [cache-stability:221-287][core] separates pure Defer/SoftPlus replay from SOFT
  replacement of rendered delta units. [transform.rs:4491-4544][soft] supplies
  m1 and other rendered units on ordinary SOFT, without replacing existing m0.

## Failure scenario

An optimization treats zero as a demand for empty output, imposes a strict
whole-prompt limit, or rerenders frozen history on an otherwise replaying pass.
Each changes behavior despite appearing to enforce a stronger budget rule.

## Timing windows and dependencies

The positive inner fit claim uses the actual production tokenizer and
representable loop arithmetic. It does not hold for an arbitrary estimator
that assigns nonzero cost to empty output. Nonfinite request handling is not
generalized into the direct renderer contract. Wrapper cost remains separate.

## What a test must construct

Compare positive, zero, and negative direct-API cases against the reference.
Create wrapper-dominated small budgets to retain the bounded outer-overrun
behavior. Compare existing m0 bytes on ordinary SOFT while allowing changed m1
and rendered units. Compare the retained prefix on pure Defer/SoftPlus. Test
HARD and pressure refold rematerialization separately, and cover [H4][h4]'s
outer retry matrix. Existing checks are
[unaudited](../existing-checks.md#history-render); no boundary matrix runs here.

## Investigation log

### Q: Can request validation replace the direct API's budget semantics?

- Sources examined: [Request resolution][validation], [inner guard][inner], and
  [outer retry][outer].
- Findings: The direct function disables the guard for nonpositive budgets;
  request resolution and outer wrapped-slice retries are different operations.
- Missing evidence: The proposed shared-budget API and its compatibility
  decisions are not supplied.
- Conclusion: Keeping these contracts separate is required; any API change needs
  human input and is not authorized by this preservation supplement.

[inner]: ../../../../crates/daemon/src/decay_render.rs#L289-L338
[tiers]: ../../../../crates/daemon/src/decay_render.rs#L203-L240
[tokenizer]: ../../../../crates/tokenizer/src/lib.rs#L123-L149
[outer]: ../../../../crates/daemon/src/m0_compose.rs#L178-L215
[validation]: ../../../../crates/daemon/src/lib.rs#L8239-L8242
[hard]: ../../../../crates/daemon/src/transform.rs#L4081-L4108
[refold]: ../../../../crates/daemon/src/transform.rs#L4348-L4381
[core]: ../../../../crates/cache-stability/src/lib.rs#L221-L287
[soft]: ../../../../crates/daemon/src/transform.rs#L4491-L4544
[h4]: ../catalog.md#history-outer-retry-pressure-is-exercised
