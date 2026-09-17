# packing-budget-is-an-integer-never-clamped

## Discovery trigger

RP2.8 acceptance rows AC7 and AC8 require a compile-time distinction between
`EmbedTokens` and `ClaudeTokens` and refuse NaN, negative, non-integer, or
absent budgets rather than clamping them; Q10 forbids inheriting the existing
render precedents.

## Evidence trail

- `crates/daemon/src/packing/mod.rs` `ClaudeTokens(u64)` with two `compile_fail`
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

## Investigation log

### Q: Where does over-range begin for a budget that arrives as an `f64`?

- Sources examined: `crates/daemon/src/packing/mod.rs` `from_budget`; the
  `budgets_are_integers_and_never_clamped` test; review thread
  [#668 r4030901776](https://github.com/ahrav/eidnara/pull/668#discussion_r4030901776).
- Findings: every integer below 2^53 is exact in an `f64`; 2^53 + 1 rounds
  to 2^53 before `from_budget` sees it, so 2^53 itself cannot be told from a
  rounded value, and `fract()` cannot tell either. The first cut at 2^64
  accepted such values.
- Missing evidence: none; the wire route that produces the `f64` lands with
  U5a, and whether it decodes an integer directly is that route's question.
- Conclusion: resolved with answer - values of 2^53 and above are
  `TooLarge`; 2^53 - 1 is the largest accepted budget.
