# memory-trim-clamps-malformed-budget-and-floors-the-integer

## Discovery trigger

Lens: claimed guarantees (P6) and existing tests. The brief named "integer
budget conversion". The required-phase catalog
(`docs/properties/selection-packing/required/catalog.md`,
`packing-budget-is-an-integer-never-clamped`) names the `.max(1.0)` clamp in
`trim_user_profile_to_budget` as a precedent the packer must not inherit;
`trim_memories_to_budget` carries the same clamp and the plan's U2 reuses
`required` for the memory trim.

## Evidence trail

- `crates/daemon/src/m0_compose.rs:79` `let budget = budget_tokens.max(1.0);`
  `f64::max` returns the non-NaN operand, so NaN becomes 1.0; negatives and
  zero become 1.0.
- `:80-83` `wrapper` estimated; `if wrapper as f64 > budget { return
  Vec::new(); }` so a budget of 1.0 returns empty for any non-zero wrapper.
- `:90` `budget.floor() as u64 - wrapper as u64`: the guard above makes the
  subtraction non-negative; `+inf.floor() as u64` saturates to `u64::MAX`
  (Rust float-to-int `as` saturates).
- Config entry: `crates/daemon/src/config.rs:1198-1214` applies
  `budget.max(1.0)` to `memory.injection_budget_tokens` and the deprecated
  key; `number_at` rejects non-numbers, so a NaN cannot arrive from
  configuration. The decisions catalog record
  `dec-a-memory-injection-budget-documented-range-has-no-implementing-code`
  owns that clamp.
- Tests: `m0_compose.rs:300` `budget_boundaries_match_the_replaced_loop`
  runs NaN, -3.0, 0.0, wrapper-1, wrapper, wrapper+10.5, wrapper+40.0,
  wrapper+40.5, wrapper+41.0, 1e30, +inf against the retained loop; `:337`
  proptest over NaN and -50..400.
- Related: `trim_user_profile_to_budget` (`:115-133`) uses `budget_tokens.max(1.0)`
  in a float comparison, the precedent the required catalog cites.

## Failure scenario

Two conversions of one configured value. U2 charges rule rows through
`reserve_required` against an integer `ClaudeTokens` and then packs optional
rows through `skip_and_continue` over "the remainder". If the required
phase refuses a fractional or over-range budget with a typed `BudgetRefusal`
while the optional phase floors and clamps the same value, one pass can
refuse rules and still serve memories, or compute a remainder from a
different integer than the one the required phase charged against.

## Timing windows and dependencies

None; pure. Depends on `f64::max` NaN semantics and `as u64` saturation,
both language-defined.

## What a test must construct

1. Direct calls: `trim(rows, f64::NAN)`, `trim(rows, -3.0)`, `trim(rows,
   0.0)` all return `[]`; `trim(rows, wrapper + 10.5) == trim(rows,
   wrapper + 10.0)`; `trim(rows, f64::INFINITY)` returns every positive row
   in order; none panics (byte estimator, `wrapper == 34`).
2. After U2: the same budget value fed to the required phase and the
   optional phase yields one integer; assert `required_limit +
   optional_budget + reserved == floor(configured)` or the typed refusal is
   shared.

## Investigation log

### Q: Under U2, does the memory trim keep the clamp or adopt the typed refusal?

- Sources examined: plan U2 step 3 ("Convert the budget to an integer
  once"); required catalog record and `crates/daemon/src/packing/mod.rs:69`
  `ClaudeTokens::from_budget` with `BudgetRefusal` at `:102` (named in that
  catalog as having no production caller);
  `m0_compose.rs:79`, `:90`.
- Findings: the plan states one conversion but names neither the clamp nor
  the refusal; the two existing conversions disagree on NaN, negative,
  fractional, and over-range inputs.
- Missing evidence: the implementation decision.
- Conclusion: needs human input.
