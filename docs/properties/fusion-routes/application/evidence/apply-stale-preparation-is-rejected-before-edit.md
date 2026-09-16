# apply-stale-preparation-is-rejected-before-edit

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
- `crates/daemon/tests/edit_receipts.rs` `a_changed_context_between_prepare_and_apply_is_stale_and_forwards_nothing`.
- `crates/daemon/tests/edit_receipts.rs` `outcomes_are_distinct_and_capacity_is_bound_before_preparation`: a `spans` item with `spn` instead of `span` is `invalid_params`, so a misspelled key cannot silently become a whole-buffer span in the digest.

## Failure scenario

An edit prepared against one context would be applied to another.

## Timing windows and dependencies

Context changes, including compaction, between prepare and apply.

## What a test must construct

A prepared receipt and a differing context body.

## Investigation log

### Q: Is the witness independent of the store's own bookkeeping?

- Sources examined: the test consumer's edit log in
  `crates/daemon/tests/edit_receipts.rs`.
- Findings: the log records every `forwarded` answer as the harness would see
  it, so the effect count is observed on the wire rather than read from the
  store.
- Missing evidence: none.
- Conclusion: resolved with answer.
