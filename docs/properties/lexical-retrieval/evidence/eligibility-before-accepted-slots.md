# eligibility-before-accepted-slots

- `admit` in `crates/retrieval/src/lexical/retrieve.rs` judges hits in kernel
  batches of `batch_rows` before accepting any.
- `crates/retrieval/tests/lexical_retrieval.rs:685` and `:719` cover an
  ineligible leader and the accepted bound inside a batch.
- `:2222` projects three equal-byte occurrences of distinct objects, hides the
  comparator's first, and asserts the other two fill both accepted slots with
  one-row batches under duplicated and permuted probes.
