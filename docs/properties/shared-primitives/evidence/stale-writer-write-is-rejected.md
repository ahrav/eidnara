# `stale-writer-write-is-rejected`

- **Discovery:** protocol-contract and consumer passes.
- **Primary evidence:** claim at `crates/lease/src/lib.rs:2-6`; SQLite enforcement in `with_conn_fenced` and `claim_fence` (`crates/storage/src/lib.rs:174-224,1299-1317`); PostgreSQL enforcement in `with_client_fenced` and `check_fence` (`primitives@89abb40`).
- **Existing evidence:** `superseded_writer_is_fenced_out_after_handover` (`crates/storage/src/lib.rs:3144-3200`) uses synthetic stores with independently supplied epochs and proves that the stale writer's fenced DDL is rejected before it creates a table. `equal_epoch_writer_is_not_fenced` (`crates/storage/src/lib.rs:3202-3220`) proves equal epochs remain authorized on the write path, and `open_claim_rejects_an_epoch_the_database_already_stores` (`crates/storage/src/lib.rs:2286-2322`) proves the open path refuses them. PostgreSQL separately checks callback rollback in `fenced_callback_error_rolls_back_rows` (`primitives@89abb40`) through synthetic stale rejection in `superseded_writer_cannot_migrate` (`primitives@89abb40`). Both backends reject a stale writer's schema SQL before it executes.
- **Failure scenario:** old connection persists after releasing lease; replacement claims newer epoch; old connection writes late.
- **Timing window:** handover through old-connection closure.
- **Instrumentation:** missing end-to-end retained-connection handover and a complete protected write-site inventory.
- **Open-question log:** unfenced SQLite consumer mutations prevent a backend-wide guarantee.
