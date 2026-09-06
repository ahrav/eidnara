# acknowledged-commit-survives-process-crash

## Discovery trigger

Task 1 asked what an acknowledged write actually promises. Tracing the promise
back from `MemoryStore::commit` reached `storage::with_conn_fenced`, and the
trail stopped at a missing declaration: no code in either crate sets
`PRAGMA synchronous`. The durability class of every acknowledged write is
therefore inherited from the dependency's build rather than chosen.

## Evidence trail

The write path, outermost to innermost:

- `crates/memory-store/src/lib.rs:7215-7223` `MemoryStore::commit` delegates to
  `commit_with_consumed_drops`, which delegates to `commit_transform`
  (`:7226-7257`).
- `lib.rs:7352` `commit_transform` opens the only transaction:
  `self.inner.with_conn_fenced(|tx| { ... })`.
- `lib.rs:7354-7357` reads the current `row_version` inside that transaction.
- `lib.rs:7390-7397` `INSERT INTO cache_state ... ON CONFLICT` upsert.
- `lib.rs:7598` returns `CommitOutcome::Committed(next)` from the closure.
- `/local/home/ahrav/scratch/commons/crates/storage/src/lib.rs:229`
  runs the closure; `:230-231` calls `tx.commit()` and maps its error.
- `lib.rs:7599` receives the result and `:7600-7608` converts the outcome.

So "acknowledged" means `tx.commit()` returned `Ok`.

What that commit is fsynced against:

- `storage:287` `conn.pragma_update(None, "journal_mode", "WAL")`.
- `storage:289` `conn.busy_timeout(Duration::from_secs(5))`.
- `storage:291` `conn.pragma_update(None, "foreign_keys", "ON")`.
- Nothing else. A content search for `synchronous` across
  `crates/memory-store/src`, `crates/context-core/src`, `crates/tokenizer/src`, and
  `storage/src/lib.rs` returns only the verifier read at
  `crates/memory-store/src/sqlite_runtime.rs:133-138` (source-catalog path, not present at HEAD), its doc comment at `:112`,
  and two lines in `crates/memory-store/tests/sqlite_runtime.rs` (source-catalog path, not present at HEAD).

The engine:

- `Cargo.toml:32` `rusqlite = { version = "0.32", features = ["bundled"] }`.
- `Cargo.lock` pins `libsqlite3-sys 0.30.1`.
- `~/.cargo/registry/src/index.crates.io-*/libsqlite3-sys-0.30.1/sqlite3/sqlite3.h:149`
  declares `#define SQLITE_VERSION "3.46.0"`.

## Failure scenario

A pass computes new `CoreState`, calls `commit`, and receives `Ok(row_version)`.
The caller treats that as durable and caches `row_version` as its next CAS
expectation. The host loses power. On restart the `cache_state` row is at the
previous `row_version` because the WAL commit frame was in the page cache but
not on stable storage. The caller's next `commit` presents an `expected` that is
now one ahead of durable state, the CAS at `lib.rs:7358-7382` returns
`CasConflict`, and the caller re-loads. That is a recoverable outcome for the
CAS itself.

The unrecoverable variant is a write whose *effect* was communicated outward
before the crash. `commit_state_import` (`lib.rs:7145-7205`) records
`state_imports` in the same transaction as the compartments, so replay is
idempotent by design. But `deliver_historian_side_channel`
(`lib.rs:9662-9718`) commits the domain insert and the delivered mark together;
if that commit is lost, the outbox row reappears as undelivered and
`insert_historian_events_tx` (`lib.rs:12388-12409`) runs a plain `INSERT` with
no `OR IGNORE` and no unique constraint on the outbox identity. Loss of the
commit is safe there (nothing was inserted either). Loss of an fsync *after* a
partial WAL flush is not a case SQLite permits, so the residual risk is
confined to whole-commit loss.

## Timing windows and dependencies

- The acknowledgement window is `storage:230` to the caller observing
  `Ok`. Under `synchronous=FULL` the fsync happens inside `commit()`.
- Under `synchronous=NORMAL` in WAL mode there is no fsync at commit; durability
  waits for the next checkpoint. No checkpoint is ever forced: no
  `wal_autocheckpoint`, no `wal_checkpoint`, no `journal_size_limit` anywhere in
  either crate. So the exposure window under `NORMAL` is up to one full
  autocheckpoint interval, which at the 1000-page default is a variable amount
  of write volume, not a bounded time.
- Depends on: the single-writer file lease (`storage:279-282`) holding,
  so no second writer is interleaving; and the process-local
  `Mutex<Connection>` (`storage:189`) serializing writes.

## What a test must construct

Two separate variants, because they prove different things.

1. **Process crash.** Open a store, commit a known row, obtain `Ok`, then
   `SIGKILL` the process from outside without unwinding. Reopen through
   `MemoryStore::open` and assert the row and its `row_version`. This is
   constructible today with a child-process harness; the existing
   `crates/memory-store/tests/` files use `tempfile::tempdir` and in-process
   reopen, which does not exercise it.
2. **Power loss.** The same shape but losing the page cache. Not constructible
   in user space. Needs `dm-flakey`, a device-mapper write-drop layer, or a VM
   snapshot. Route to `/testing:crash-consistency-and-failpoint-testing`.

The oracle for both is per-identity, not aggregate: assert the specific
`session_id` row's `row_version` equals the acknowledged value, since a total
row count can be satisfied by an unrelated row.

## Investigation log

### Q: Does the `libsqlite3-sys 0.30.1` bundled build override `SQLITE_DEFAULT_SYNCHRONOUS`?

- Sources examined: `Cargo.toml:24-32`, `Cargo.lock` entry for
  `libsqlite3-sys`, the vendored `sqlite3/sqlite3.h` in the registry checkout,
  content search for `synchronous` across both crates.
- Findings: the vendored header confirms SQLite 3.46.0. No `PRAGMA synchronous`
  is issued by any code that reaches this connection. Upstream SQLite defaults
  `SQLITE_DEFAULT_SYNCHRONOUS` to 2 (`FULL`).
- Missing evidence: I did not read `libsqlite3-sys`'s `build.rs` or its
  `bindgen_bundled_version.rs` for a `-DSQLITE_DEFAULT_SYNCHRONOUS` flag, and I
  did not execute `PRAGMA synchronous` against a built connection.
- Conclusion: unresolved, needs one query against a connection produced by
  `MemoryStore::open`. The inference is `FULL`, but this lens does not state it as
  fact.

### Q: Is there any forced checkpoint or fsync at shutdown?

- Sources examined: content search for `wal_checkpoint`, `wal_autocheckpoint`,
  `journal_size_limit` across `crates/` and `storage/src/lib.rs`;
  `MemoryStore` has no `Drop` impl (`lib.rs:4610-4634` is a plain struct with no
  `impl Drop` in the file).
- Findings: none found. `SqliteStore` holds `conn: Mutex<Connection>` and a
  `_lease` field (`storage:322-326`); dropping it closes the
  connection, which SQLite checkpoints on last-connection close by default.
- Missing evidence: whether the process ever drops the store on the shutdown
  path, or exits with the connection still open.
- Conclusion: resolved with answer — no explicit checkpoint policy exists;
  durability relies entirely on SQLite's commit-time behaviour, which is what
  makes the unset `synchronous` level load-bearing.
