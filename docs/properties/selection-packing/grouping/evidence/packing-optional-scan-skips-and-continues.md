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
  same shape through the daemon entry over the budget the required phase left:
  three groups whose rendered-delta costs are big, small, medium with a budget
  of small plus medium, so the first is skipped and the two behind it admitted.

## Failure scenario

A prefix packer stops at the first group that does not fit and leaves budget
unused while smaller later groups are never visited.

## Timing windows and dependencies

None.

## What a test must construct

- Costs 11, 4, 6 with remaining budget 10 through the pure function; through
  the daemon entry, three groups whose rendered costs are big, small, medium
  with a budget of small plus medium.
- Generated cost lists and budgets compared against the reference.
