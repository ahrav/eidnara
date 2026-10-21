# packing-reserve-is-computed-once-after-wrapper-and-disclosure

## Discovery trigger

Plan KTD5 ("charged untruncated through `admit_required` / `reserve_required`
against `min(rules_reserve, budget - wrapper - disclosure)`"; "Disclosure
`<omitted-rules count="N"/>` is reserved before packing so it is never a
victim") and U2 step 3 ("Convert the budget to an integer once; reserve
wrapper and disclosure tokens"). Serves R10, R11, KTD5; sibling of
`packing-budget-is-an-integer-never-clamped` in
`docs/properties/selection-packing/required/catalog.md:144`.

## Evidence trail

- `crates/daemon/src/m0_compose.rs:79-90`: `let budget = budget_tokens.
  max(1.0)`; wrapper estimated from `"<project-memory>\n</project-memory>"`;
  `if wrapper as f64 > budget { return Vec::new(); }`; then
  `budget.floor() as u64 - wrapper as u64` is the scan budget. One
  conversion, wrapper first, subtraction safe because of the guard.
- `crates/daemon/src/m0_compose.rs:94-103`: the marginal cost charges each
  category's open and close tags once; this cost model must stay identical
  across the two phases or a required row's cost differs from what the
  optional phase would have charged.
- `crates/daemon/src/config.rs:1198-1201`: `memory_budget_tokens` is an f64
  clamped to `max(1.0)` at config time; a NaN survives `max` as NaN in Rust
  (`f64::max` returns the non-NaN operand, so `NaN.max(1.0) == 1.0`; the
  clamp is sound).
- `crates/retrieval/src/packing/required.rs:13-31`: `TokenCount` for `u64`
  with checked arithmetic; the plan's integer domain.
- `crates/daemon/src/m0_compose.rs:300-345`: existing tests pin the
  wrapper-only reservation against the replaced loop.
- `crates/daemon/src/token_cache.rs:293` (`cached_estimate_tokens`, used at
  `canonical_memory.rs:208`): the estimator is shared; the disclosure line
  cost must be estimated by the same function.

## Failure scenario

Legal witness: `B = 4000.7`, `w = 7`, `d = 9`, `r = 1600`. `floor(B) = 4000`;
`remaining = 3984`; `required_limit = 1600`; after charging 1200 of rules,
`optional_budget = 2784`.

Smallest violating traces:
1. Two conversions: the required phase floors `B` and the optional phase
   receives `B - charged` as f64 and floors again after subtracting a
   fractional estimate; the block admits one token more than `floor(B)` in
   some inputs. The existing catalog's "integer never clamped" record is the
   primitive-level statement of why.
2. Disclosure not reserved: the required phase charges against
   `min(r, floor(B) - w)`; rules fill exactly that; the optional phase gets
   zero; the disclosure line is then appended and the block exceeds the
   budget by `d`, or the line is dropped to fit and omission is silent.
3. Underflow: `B = 10`, `w = 7`, `d = 9`; `floor(B) - w - d` underflows an
   unsigned subtraction. The guard must saturate (zero rules, no disclosure
   line since none can be rendered) or return the empty block.
4. Reserve applied before wrapper: `required_limit = min(r, floor(B))` with
   `r = 4000`; rules consume the whole budget and the wrapper does not fit;
   the renderer still emits the wrapper and the block overruns.

## Timing windows and dependencies

- None; pure arithmetic per pass.
- Depends on `packing-omitted-count-is-disclosed-reported-and-digested` for
  the disclosure line's exact bytes (its cost `d` is estimated from the
  rendered line for the maximum plausible `N`, or for the actual `N` if the
  packer iterates; pin one).

## What a test must construct

1. A pure function `budget_plan(budget_f64, wrapper, disclosure, reserve)
   -> {required_limit, optional_base}` with a table:
   - `(4000.7, 7, 9, 1600) -> (1600, 3984)`;
   - `(4000.0, 7, 9, 10_000) -> (3984, 3984)`;
   - `(0.0, 7, 9, 1600)`: budget clamps to 1; empty block;
   - `(NaN, ...)`: same as 1;
   - `(15.0, 7, 9, 1600)`: `remaining = 0` (saturating); no rules; whether
     the disclosure line renders when `N > 0` but `d` did not fit is pinned
     (expected: empty block, since the wrapper plus line exceed the budget).
2. A proptest: for all finite `B >= 1`, rendered block tokens
   `<= floor(B)` including the disclosure line when present.
3. Assert the token estimator used for `d` is `cached_estimate_tokens`.

## Investigation log

### Q: Is `d` charged when no required candidate exists?
- Sources examined: plan KTD5 and U2 step 3.
- Findings: "reserved before packing so it is never a victim" does not say
  whether the reservation is conditional on candidates existing. Charging
  it always costs `d` tokens of optional memory on every pass with no rules.
- Missing evidence: the implementation's choice.
- Conclusion: resolvable at implementation; either choice is consistent with
  the plan; the test table must pin one.

### Q: Is the disclosure cost estimated for the actual `N` or a bound?
- Sources examined: plan text; `cached_estimate_tokens` signature.
- Findings: `N` is known only after the required phase, which needs `d`
  first. A fixed upper-bound string (for example `count="9999"`) or a
  two-step compute is required.
- Conclusion: resolvable at implementation; a fixed-width bound is the
  simpler deterministic choice.
