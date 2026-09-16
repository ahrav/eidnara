# apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement

## Discovery trigger

The RP2.7 specification's application lifecycle section and the RP2.7.U4
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u3c-dense-lane` at
`f318c4a4`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/edit_receipts.rs`: `ReceiptStore::prepare`, `apply`, and
  `confirm` implement the state machine; `ReceiptLimits::validate` refuses an
  unapproved retention; the three handlers bind scope through
  `kernel_request` and answer `disabled` without a limit set.
- `docs/host-wire-protocol.md` Section 7.8 fixes the literals.
- `crates/daemon/tests/edit_receipts.rs` `outcomes_are_distinct_and_capacity_is_bound_before_preparation`.

## Failure scenario

A harness could report an outcome the daemon cannot classify, or an empty replacement could pass as `keep`.

## Timing windows and dependencies

None.

## What a test must construct

A live daemon with limits installed.

## Investigation log

### Q: Is the witness independent of the store's own bookkeeping?

- Sources examined: the test consumer's edit log in
  `crates/daemon/tests/edit_receipts.rs`.
- Findings: the log records every `forwarded` answer as the harness would see
  it, so the effect count is observed on the wire rather than read from the
  store.
- Missing evidence: none.
- Conclusion: resolved with answer.
