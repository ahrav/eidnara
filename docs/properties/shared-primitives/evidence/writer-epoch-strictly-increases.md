# `writer-epoch-strictly-increases`

- **Discovery:** data-integrity, protocol, and wildcard passes.
- **Primary evidence:** contract at `crates/lease/src/lib.rs:2-6`; `read_epoch`, `bump_epoch_above`, and `persist_epoch` provide bounded parsing, checked increment above persisted state and an optional resource floor, and fixed-width update.
- **Contradictory code evidence:** no stable-storage sync supports a machine-power-loss claim; exact partial-I/O behavior is untested.
- **Existing evidence:** `exclusive_epoch_exceeds_resource_floor` and SQLite's `database_epoch_survives_repeated_lease_sidecar_loss` cover resource-floor recovery. `invalid_epoch_states_fail_closed` and `interrupted_persist_never_leaves_a_lower_parseable_epoch` cover invalid, exhausted, and ordered prefix-write cases; clean acquisition tests cover process-local persistence.
- **Failure scenario:** machine power loss or restoration of an older valid file can still cause a non-increasing returned token.
- **Instrumentation:** missing external maximum-ever-returned witness per physical root and key tuple.
- **Open-question log:** no repair protocol exists for an older valid file or power-loss regression.
