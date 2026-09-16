# apply-idempotency-key-binds-tuple-and-dedups-by-digest

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
- `crates/daemon/tests/edit_receipts.rs` `same_key_and_digest_replays_the_known_outcome_with_one_effect`.
- `crates/daemon/src/edit_receipts.rs` `a_changed_digest_against_an_unknown_receipt_is_a_conflict`: a changed digest against an `unknown` receipt is `conflict`, the same as against an in-flight or complete one.

## Failure scenario

Two intents over one tuple would collide, or a retry would forward a second edit.

## Timing windows and dependencies

A duplicate arriving while the apply is in flight.

## What a test must construct

Two prepares over one tuple; a changed span after a forward.

## Investigation log

### Q: Is the witness independent of the store's own bookkeeping?

- Sources examined: the test consumer's edit log in
  `crates/daemon/tests/edit_receipts.rs`.
- Findings: the log records every `forwarded` answer as the harness would see
  it, so the effect count is observed on the wire rather than read from the
  store.
- Missing evidence: none.
- Conclusion: resolved with answer.
