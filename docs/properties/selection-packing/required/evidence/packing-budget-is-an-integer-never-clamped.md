# packing-budget-is-an-integer-never-clamped

## Discovery trigger

RP2.8 acceptance rows AC7 and AC8 require a compile-time distinction between
`EmbedTokens` and `ClaudeTokens` and refuse NaN, negative, non-integer, or
absent budgets rather than clamping them; Q10 forbids inheriting the existing
render precedents.

## Evidence trail

- `crates/daemon/src/packing.rs` `ClaudeTokens(u64)` with two `compile_fail`
  doctests; `from_budget` refuses each malformed shape with a `BudgetRefusal`
  variant.
- `crates/daemon/src/m0_compose.rs` `trim_user_profile_to_budget` clamps
  `budget_tokens.max(1.0)`; the packer test runs it with NaN, negative, and
  negative-infinite budgets and shows it answering as if the budget were one.
- `crates/retrieval/src/packing/required.rs` `TokenCount` is the integer
  trait the pure layer sums through `checked_add`.

## Failure scenario

A NaN budget clamped to one token silently drops every optional item and most
required ones; a fractional budget rounded up emits over-budget bytes.

## Timing windows and dependencies

None.

## What a test must construct

- Each malformed budget value.
- The legacy clamp as a negative control that must answer, not refuse.
- The two cross-substitution doctests under the all-features doctest job.
