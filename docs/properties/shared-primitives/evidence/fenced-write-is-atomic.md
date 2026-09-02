# `fenced-write-is-atomic`

- **Discovery:** storage fenced-write pass.
- **Primary evidence:** `with_conn_fenced` (`crates/storage/src/lib.rs:170-216`) claims the fence and runs the callback inside one `IMMEDIATE` transaction and commits only after the callback returns `Ok`.
- **Existing evidence:** `fenced_write_rolls_back_on_error` (`crates/storage/src/lib.rs:2440-2467`) uses a store whose epoch is ahead of the stored fence, inserts a row, returns `Err`, and asserts both the row and the fence epoch are unchanged; `fenced_write_commits_and_persists` (`crates/storage/src/lib.rs:2022-2037`) covers the commit path across reopen.
- **Failure scenario:** a claim committed separately from the callback would advance the fence for a write that never landed.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The rollback test checks the fence epoch as well as the domain row, which distinguishes one transaction from two.
- **Open-question log:** none.
