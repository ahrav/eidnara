# rebill-total-checked-multiplication

## Discovery trigger
Parent specification #1119 C16, user story 6; Part B record `rebill-total-checked-multiplication`;
implemented by #1124.

## Evidence trail
- `crates/eval-core/src/campaign.rs:113` `PrefixRebill`; `:125` `total` widens
  to `u64` and uses `checked_mul` twice.
- `crates/eval-core/src/report.rs:378` `check_rebill` refuses an overflowing
  record by arm.
- `crates/eval-core/tests/report.rs::the_prefix_rebill_total_is_the_checked_product`.

## Failure scenario
`u32::MAX` writes times `u32::MAX` sessions times `2^30` tokens leaves `u64`;
a wrapping multiply would report a small cost.

## Timing windows and dependencies
None.

## What a test must construct
Inputs whose product exceeds `u64`, inside the canonical safe range field by
field.

Legal witness: `{3, 4, 1000}.total() == Ok(12000)`. Smallest falsifier: `{2,
1, u64::MAX}.total() == Ok(u64::MAX - 1)`.

## Investigation log
No open questions were raised while verifying this record.
