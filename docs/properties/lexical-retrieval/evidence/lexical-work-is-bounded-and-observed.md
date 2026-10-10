# lexical-work-is-bounded-and-observed

- `Consumed` in `crates/retrieval/src/lexical/retrieve.rs` reports probes,
  counted rows, scanned rows, ranked matches, judgments, batches, exclusions,
  and `sql_steps`, the `SQLITE_STMTSTATUS_VM_STEP` operations of the count,
  scan, and lookup statements for the probes that finished.
- FTS5's rank function reads each ranked match's document size through a
  statement FTS5 keeps internal, so `sql_steps` excludes that part of scoring;
  `ranked_matches` bounds it.
- `crates/retrieval/tests/lexical_retrieval.rs:2381` asserts the counters at
  the scan bound and one row below it, the judgments and batches at the bound,
  equal work under a result cap of one, and SQL steps that grow with one more
  matching row on a ranked run and on a common run.
- `crates/retrieval/tests/lexical_retrieval.rs:2542` ends a ranked probe's
  scan bound at a rank change. A tied group of 320 rows past the bound runs the
  same SQL steps as 320 rows with one rank each, because one row past the bound
  shows truncation.
- `crates/retrieval/tests/lexical_retrieval.rs:2312` records the retrieval
  call's Rust allocations with the daemon's `alloc_recorder` after a warm-up
  run. One row below the scan bound allocates fewer events and bytes. After 32
  more matching rows, a common scan keeps its counted rows, scanned rows, SQL
  steps, and allocation events, while a ranked run scores all 36 matches inside
  `rank_budget` and allocates more. SQLite's allocator is outside the recorder.
