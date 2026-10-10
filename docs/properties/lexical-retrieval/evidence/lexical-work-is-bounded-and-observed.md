# lexical-work-is-bounded-and-observed

- `Consumed` in `crates/retrieval/src/lexical/retrieve.rs` reports probes,
  counted rows, scanned rows, ranked matches, judgments, batches, exclusions,
  and `sql_steps`, the `SQLITE_STMTSTATUS_VM_STEP` operations of the count and
  scan statements for the probes that finished.
- A ranked probe's FTS5 rank sort runs in a nested statement SQLite keeps
  internal, so `sql_steps` excludes its scoring; `ranked_matches` bounds it.
- `crates/retrieval/tests/lexical_retrieval.rs:2281` asserts the counters at
  the scan bound and one row below it, the judgments and batches at the bound,
  equal work under a result cap of one, and SQL steps that grow with one more
  matching row on a ranked run and on a common run.
- No test counts allocations.
