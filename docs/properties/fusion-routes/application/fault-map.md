# Application fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: `rp27/u3c-dense-lane` at
`f318c4a4`.

| Fault or state | Available seam | Records |
| --- | --- | --- |
| Duplicate apply while in flight | A second `retrieval.apply` before any confirm. | apply-idempotency-key-binds-tuple-and-dedups-by-digest, apply-selection-preparation-application-are-distinct-states |
| Context change between prepare and apply | A differing context body on `retrieval.apply`. | apply-stale-preparation-is-rejected-before-edit |
| Daemon restart after forward or after prepare | A second `KernelDaemon`; the first is shut down. `set_edit_receipt_limits(None)` then a reinstall on one daemon. | apply-unknown-outcome-never-replays-blindly |
| Lost acknowledgment | `retrieval.confirm` with `applied_identity: null`. | apply-unknown-outcome-never-replays-blindly, apply-daemon-receipt-does-not-mark-harness-edit-applied |
| Stale acknowledgment | `retrieval.confirm` naming another forwarded identity. | apply-daemon-receipt-does-not-mark-harness-edit-applied |
| Count eviction | `max_keys = 2` on a live daemon; every receipt in flight; an unknown receipt under `max_keys = 1`; `max_keys` narrowed from 8 to 3 with synthetic `Instant`s in the module tests. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Time expiry | Synthetic `Instant`s passed to `ReceiptStore` in the module tests. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Unapproved limits | `retention` below `RETENTION_FLOOR` passed to `set_edit_receipt_limits`; `max_keys` above `MAX_KEYS_CEILING` passed to `ReceiptLimits::validate`. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Forged read-back | A malformed, uppercase, or mismatched applied identity on confirm, or a key without the minted shape, on one daemon and after a restart. | apply-daemon-receipt-does-not-mark-harness-edit-applied, apply-unknown-outcome-never-replays-blindly |
| Unrecordable read-back | A well-formed foreign read-back over a project whose only receipt is in flight, in the module tests. | apply-unknown-outcome-never-replays-blindly |
| Cross-project key | A second route bound to another project root on one `KernelDaemon`, driven through `dispatch_value_for_test`. | apply-receipt-belongs-to-the-project-that-prepared-it |
| Misspelled span field | A `spans` item carrying `spn` instead of `span` on `retrieval.apply`. | apply-stale-preparation-is-rejected-before-edit |
| Over-capacity edit | `edit_bytes` one over the allowance or capacity. | apply-append-allowance-and-replacement-capacity-are-bound-before-preparation |
| Outcome outside the set | `outcome: "applied"` or `"unknown"` on confirm. | apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement |
| Backend answer changes after bind | A `CapabilitySource` behind a `Mutex` the test rewrites. | apply-context-capabilities-default-closed-per-harness |
| No declaration | A `KernelDaemon` started without a capability source. | apply-context-capabilities-default-closed-per-harness |
| Advertising consumer strings | `StartOptions::consumer_capabilities` on a `pi` route. | apply-consumer-capability-strings-never-authorize-edits |
| Partial or span-level survivors | `survivors` sets on `retrieval.prepare` with action `suppress`. | apply-suppression-requires-confirmed-surviving-span |

The store takes `now: Instant` on every operation, so the module tests drive
the time bound with synthetic instants and no sleep.
