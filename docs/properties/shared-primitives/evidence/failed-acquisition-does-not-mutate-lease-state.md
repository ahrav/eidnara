# `failed-acquisition-does-not-mutate-lease-state`

- **Discovery:** targeted pre-lock side-effect pass after portfolio evaluation.
- **Primary evidence:** file-backed exclusive and shared acquisition call `open_lease_file`, including descriptor hardening, before attempting the lock. PostgreSQL tries its session advisory lock before creating infrastructure tables or bumping the epoch (`commons@89abb40 crates/`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:242-261`).
- **Existing evidence:** file contention tests assert returned errors and later acquisition behavior, not file bytes or metadata.
- **Failure scenario:** losing acquirer changes mode or creates the file before returning `Held`; foreign ownership or read-only access returns undifferentiated `Io` before contention is known.
- **Timing window:** incumbent live; competitor reaches hardening before try-lock.
- **Instrumentation:** content, mode, owner, and mtime snapshot around rejected acquisition.
- **Open-question log:** commit `49bcaa2` assumes a single-account host but the public crate contract does not state that precondition.
