# `logical-store-has-single-lease-identity`

- **Discovery:** targeted consumer-boundary pass after portfolio evaluation.
- **Primary evidence:** lease key excludes database path (`crates/storage/src/lib.rs:80-86`); root is the database parent (`:570-585`).
- **Existing evidence:** `distinct_databases_do_not_falsely_contend` (`:1057-1065`) uses different parent directories, so it cannot expose sibling-file aliasing.
- **Failure scenario:** sibling databases with equal key fields share one lease; one SQLite database opened under differing module/namespace descriptors or cross-parent symlink/hardlink aliases gets split leases.
- **Timing window:** concurrent opens are needed for writer overlap; false contention needs no timing.
- **Instrumentation:** missing authoritative logical-store ID and canonical `(root,key)` observation.
- **Open-question log:** no validation binds one database path to one immutable descriptor. Deployment authority remains external.
