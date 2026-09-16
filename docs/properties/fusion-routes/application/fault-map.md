# Application fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: `rp27/u3c-dense-lane` at
`f318c4a4`.

| Fault or state | Available seam | Records |
| --- | --- | --- |
| Duplicate apply while in flight | A second `retrieval.apply` before any confirm. | apply-idempotency-key-binds-tuple-and-dedups-by-digest, apply-selection-preparation-application-are-distinct-states |
| Context change between prepare and apply | A differing context body on `retrieval.apply`. | apply-stale-preparation-is-rejected-before-edit |
| Daemon restart after forward or after prepare | A second `KernelDaemon`; the first is shut down. | apply-unknown-outcome-never-replays-blindly |
| Lost acknowledgment | `retrieval.confirm` with `applied_identity: null`. | apply-unknown-outcome-never-replays-blindly, apply-daemon-receipt-does-not-mark-harness-edit-applied |
| Stale acknowledgment | `retrieval.confirm` naming another forwarded identity. | apply-daemon-receipt-does-not-mark-harness-edit-applied |
| Count eviction | `max_keys = 2` on a live daemon; every receipt in flight. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Time expiry | Synthetic `Instant`s passed to `ReceiptStore` in the module tests. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Unapproved retention | `retention` below `RETENTION_FLOOR` passed to `set_edit_receipt_limits`. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Forged read-back | A malformed or mismatched applied identity on confirm, on one daemon and after a restart. | apply-daemon-receipt-does-not-mark-harness-edit-applied, apply-unknown-outcome-never-replays-blindly |
| Over-capacity edit | `edit_bytes` one over the allowance or capacity. | apply-append-allowance-and-replacement-capacity-are-bound-before-preparation |
| Outcome outside the set | `outcome: "applied"` or `"unknown"` on confirm. | apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement |

The store takes `now: Instant` on every operation, so the module tests drive
the time bound with synthetic instants and no sleep.
