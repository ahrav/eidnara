# `stale-writer-write-is-rejected`

- **Discovery:** protocol-contract and consumer passes.
- **Primary evidence:** claim at `crates/lease/src/lib.rs:2-6`; SQLite enforcement in `with_conn_fenced` and `claim_fence` (`crates/storage/src/lib.rs:189-235,708-728`); PostgreSQL enforcement in `with_client_fenced` and `check_fence` (`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:130-154,217-238`).
- **Existing evidence:** `superseded_writer_is_fenced_out_after_handover` (`crates/storage/src/lib.rs:1990-2028`) uses synthetic stores with independently supplied epochs. `equal_epoch_writer_is_not_fenced` (`crates/storage/src/lib.rs:2064-2084`) proves equal epochs remain authorized on the write path, and `open_claim_rejects_an_epoch_the_database_already_stores` (`crates/storage/src/lib.rs:1190-1226`) proves the open path refuses them. PostgreSQL separately checks callback rollback in `fenced_callback_error_rolls_back_rows` (`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1086-1119`) through synthetic stale rejection in `superseded_writer_cannot_migrate` (`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1178-1213`). Both backends reject stale migrations before schema SQL.
- **Failure scenario:** old connection persists after releasing lease; replacement claims newer epoch; old connection writes late.
- **Timing window:** handover through old-connection closure.
- **Instrumentation:** missing end-to-end retained-connection handover and a complete protected write-site inventory.
- **Open-question log:** unfenced SQLite consumer mutations prevent a backend-wide guarantee.
