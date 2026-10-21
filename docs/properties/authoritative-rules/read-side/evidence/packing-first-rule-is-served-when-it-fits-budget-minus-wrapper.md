# packing-first-rule-is-served-when-it-fits-budget-minus-wrapper

## Discovery trigger

Plan requirement R10 ("the first rule is always served when it fits the
whole budget") and KTD5 ("Minimum service: the first rule is served when it
fits `budget - wrapper`"); U2 test scenarios "A single rule larger than the
reserve but within the whole budget is served alone" and "A single rule
larger than the whole budget yields only the disclosure line". Serves KTD5.

## Evidence trail

- `crates/daemon/src/m0_compose.rs:81-83`: today, a wrapper larger than the
  budget yields an empty set; `:91-103` an oversize row is skipped and
  smaller rows still pack. No minimum-service rule exists.
- `crates/daemon/src/memory_render.rs:111-118`: `memory_line_content` cuts
  content at 64 KiB; the "untruncated" charge of a rule is of the rendered
  line, which is itself capped at 64 KiB. A rule longer than 64 KiB is
  charged for its first 64 KiB only (see
  `cf-memory-credit-requires-admitted-visible-content` in
  `docs/properties/compression-fidelity/delivery/catalog.md:182` for the
  qualifier-truncation hazard).
- `crates/daemon/src/config.rs:1198-1201`: budget floor is 1.0; the wrapper
  alone (7 tokens by the shared estimator; the exact count is estimator
  dependent) exceeds a budget of 1, so the minimum-service bound is
  reachable at the small end.
- Plan OQ6: default reserve 1600 of 4000; the gap 1600 < cost <= 3993 is the
  minimum-service band under defaults.

## Failure scenario

Legal witness: one directive of 2,400 tokens, budget 4000, wrapper 7,
disclosure 9, reserve 1600. `required_limit = 1600`; the first row does not
fit the limit; `2400 <= 4000 - 7`, so it is served alone; `rules_omitted =
0`; optional phase receives `4000 - 7 - 9 - 2400 = 1584`.

Second witness: one directive of 5,000 tokens. Not served;
`rules_omitted = 1`; block is `<project-memory>\n<omitted-rules
count="1"/>\n</project-memory>`.

Smallest violating traces:
1. Rule applied to index 1: two rules `[2400, 2400]`; the first is served
   under minimum service; the second also "fits `budget - wrapper`" in
   isolation and is served too; the block is 4,800 tokens and overruns the
   budget. Minimum service is a guard on the first row only.
2. Rule ignored: `[2400]`; `required_limit = 1600`; the packer stops at the
   first `OverBudget` without the exception; zero rules served; the
   highest-authority row is hidden forever under a reserve smaller than one
   directive.
3. Bound mistaken: the first rule of 3,995 tokens is served under a check
   against `floor(budget)` instead of `floor(budget) - wrapper`; the block
   is 4,002 tokens.
4. Disclosure displaced: `[3990]`, budget 4000, wrapper 7, disclosure 9;
   the rule fits `budget - wrapper` (3993) and is served; a second rule is
   omitted; the disclosure line (9 tokens) no longer fits. Either the line
   is rendered and the block is 4,006 tokens, or omission is silent. The
   minimum-service bound must be `budget - wrapper - disclosure` when
   `n > 1`, or the plan's bound stands and the overrun is accepted (open
   question, resolvable at implementation).

## Timing windows and dependencies

- None.
- Depends on the order record (which row is "first") and the reserve
  record (the limit and the wrapper and disclosure costs).

## What a test must construct

1. Pure packer tests:
   - `[2400]`, limit 1600, `floor(B) - w = 3993`: served 1, omitted 0.
   - `[5000]`: served 0, omitted 1.
   - `[2400, 2400]`: served 1, omitted 1.
   - `[1500, 200]`, limit 1600: served 1, omitted 1 (ordinary prefix rule;
     minimum service does not apply because the first row fits the limit).
   - `[3990, 100]` with `d = 9`: pin the chosen bound and assert the
     rendered block token count is at most `floor(B)`.
2. Integration: seed one 2,400-token directive and three ordinary memories
   under defaults; assert the directive is in the block and at least one
   ordinary memory fits the remainder.
3. Assert `rules_served + rules_omitted == 1` in the single-rule cases.

## Investigation log

### Q: Does minimum service interact with the 64 KiB content cap?
- Sources examined: `memory_render.rs:111-118`; `m0_compose.rs:95`.
- Findings: the charged cost is of the rendered line, which is already
  capped; a rule over 64 KiB is served truncated if the capped line fits.
  "Charged untruncated" in the plan means no second truncation by the
  packer, not that the renderer's cap is lifted.
- Conclusion: resolved with answer - the renderer cap applies before the
  charge; a directive over 64 KiB is an N1 input-bound question (the plan
  bounds directive text at the decision summary limit).
