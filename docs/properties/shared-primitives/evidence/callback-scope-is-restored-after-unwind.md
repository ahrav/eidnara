# `callback-scope-is-restored-after-unwind`

- **Discovery:** storage callback-capability pass.
- **Primary evidence:** `CallbackScope::read_only` restores `query_only` to the value it found when scope installation fails after the pragma was set, so a failed schema snapshot cannot strand the shared connection read-only with no guard to undo it.  `CallbackScope::drop` (`crates/storage/src/lib.rs:539-544`) restores the connection when `release` did not run; `with_conn` recovers a poisoned mutex with `into_inner` (`crates/storage/src/lib.rs:168`).
- **Existing evidence:** `a_panicking_read_does_not_strand_the_connection_read_only` (`crates/storage/src/lib.rs:3119-3139`) panics inside `with_conn`, catches the unwind, then performs a fenced write and a maintenance statement on the same store.
- **Failure scenario:** a leaked `query_only` turns every later fenced write into `SQLITE_READONLY` for the process lifetime.
- **Timing window:** the unwind path.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The follow-up write succeeds only if both the pragma and the authorizer were cleared, so the test observes the restore through its effect.
- **Open-question log:** none.
