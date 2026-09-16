# packing-optional-scan-skips-and-continues

## Discovery trigger

RP2.8 KTD3 makes stable skip-and-continue in fused order the baseline and
prefix packing an ablation; acceptance row AC5 fixes budget 10 against costs
11, 4, 6 and requires a frozen-reference differential and a failing prefix-
packer control.

## Evidence trail

- `crates/retrieval/src/packing/scan.rs` `skip_and_continue` visits each item
  once, subtracts a fitting cost, and continues past a non-fitting one.
- `crates/daemon/src/m0_compose.rs` `trim_memories_to_budget` delegates to the
  same function, so the memory-trim precedent and the packer share one rule.
- `crates/retrieval/tests/support/frozen_packer.rs` `scan` and `prefix_scan`
  are the reference and the ablation.
- `crates/retrieval/tests/packing_grouping.rs`
  `the_scan_skips_and_continues_and_the_prefix_packer_does_not` and the
  differential proptest; `crates/daemon/tests/packing_optional.rs` runs the
  same costs through the daemon entry over the budget the required phase left.

## Failure scenario

A prefix packer stops at the first group that does not fit and leaves budget
unused while smaller later groups are never visited.

## Timing windows and dependencies

None.

## What a test must construct

- Costs 11, 4, 6 with remaining budget 10, through the pure function and the
  daemon entry.
- Generated cost lists and budgets compared against the reference.

## Investigation log

### Q: Does the memory trim share the rule or carry a copy of it?

- Sources examined: `crates/daemon/src/m0_compose.rs` `trim_memories_to_budget`
  (`use retrieval::packing::skip_and_continue`; the scan is called with the
  floored budget less the wrapper cost and a `marginal` closure that charges a
  category wrapper once, when the first row of that category is admitted);
  its `skip_and_continue_delegation_matches_the_replaced_loop` test, which
  keeps the replaced loop as the reference model.
- Findings: one implementation; the trim's remaining float clamp
  (`budget_tokens.max(1.0)`) sits before the integer scan and is the
  `required/` part's negative control, not this record's subject.
- Missing evidence: none.
- Conclusion: resolved with answer - shared rule; the clamp stays documented
  under `packing-budget-is-an-integer-never-clamped`.

### Q: Is the `default-production` reachability real at this base?

- Sources examined: `grep -rn trim_memories_to_budget crates --include=*.rs`;
  `crates/daemon/src/canonical_memory.rs`, which calls it while building the
  canonical memory snapshot; `crates/daemon/src/lib.rs` `project_memory_read`,
  which runs that read under `cfg.memory_enabled`;
  `crates/daemon/src/config.rs`, whose default sets it `true`; `grep -rn
  prepare_optional crates --include=*.rs`.
- Findings: the memory path reaches the rule on every snapshot while memory is
  enabled, which is the default; the optional packer's use has no production
  caller until RP2.8 U4 lands the call site.
- Missing evidence: none for the memory path.
- Conclusion: resolved with answer - `default-production` through the memory
  trim behind a default-on switch; the record says which path carries it.

### Q: Does any caller rely on `marginal` seeing the admitted prefix?

- Sources examined: the two callers of `skip_and_continue`: the memory trim's
  closure, which reads `admitted[seen_admitted..]` to open category wrappers,
  and `prepare_optional`, whose closure ignores the prefix and returns
  `costs[index]`.
- Findings: the memory trim depends on it; the packer does not, because each
  group's cost is summed from its ranges before the scan and no wrapper is
  shared between groups at this base.
- Missing evidence: the RP2.8 accounting profile that may introduce shared
  wrappers for the packer (U5a).
- Conclusion: unresolved, needs the accounting part - the signature is kept
  for the memory trim; whether the packer's closure grows a shared-wrapper
  term is decided when the profile lands.
