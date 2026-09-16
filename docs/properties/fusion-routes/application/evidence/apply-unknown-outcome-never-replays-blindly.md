# apply-unknown-outcome-never-replays-blindly

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
- `crates/daemon/tests/edit_receipts.rs` `a_restart_leaves_forwarded_and_unforwarded_keys_unknown_until_read_back`.
- `crates/daemon/tests/edit_receipts.rs` `a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict`.
- `crates/daemon/src/edit_receipts.rs` `a_foreign_key_completes_only_on_a_well_formed_matching_read_back_and_stays_complete`: an uppercase pair, a key without the minted shape, and a mismatched applied identity leave `unknown`; a recorded read-back later confirmed with another outcome, no applied identity, or another applied identity is `conflict`.
- `crates/daemon/src/edit_receipts.rs` `a_read_back_the_store_cannot_record_is_refused_rather_than_answered_complete`: a read-back over a project whose only receipt is in flight is `receipt_unavailable` and the key stays `unknown`.

## Failure scenario

A retry after a crash would append or replace twice.

## Timing windows and dependencies

Daemon restart after forward, restart after prepare, lost acknowledgment.

## What a test must construct

A second `KernelDaemon`; a confirm with `applied_identity: null`.

## Investigation log

### Q: Is the witness independent of the store's own bookkeeping?

- Sources examined: the test consumer's edit log in
  `crates/daemon/tests/edit_receipts.rs`.
- Findings: the log records every `forwarded` answer as the harness would see
  it, so the effect count is observed on the wire rather than read from the
  store.
- Missing evidence: none.
- Conclusion: resolved with answer.
