# zero-terms-run-no-match

- `scan` in `crates/retrieval/src/lexical/retrieve.rs` sets `Completion::Empty`
  for zero probes and runs no statement.
- `crates/retrieval/tests/lexical_retrieval.rs:582` and
  `crates/retrieval/tests/lexical_engine.rs:268` pair a zero-atom request with
  a nonempty control.
