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
| Count or time eviction | `max_keys = 2`; a 50 ms retention and a sleep. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Unapproved retention | `retention` below `retention_floor` passed to `set_edit_receipt_limits`. | apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay |
| Over-capacity edit | `edit_bytes` one over the allowance or capacity. | apply-append-allowance-and-replacement-capacity-are-bound-before-preparation |
| Outcome outside the set | `outcome: "applied"` or `"unknown"` on confirm. | apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement |

Real durations are used for the time bound because the store reads
`std::time::Instant`.
