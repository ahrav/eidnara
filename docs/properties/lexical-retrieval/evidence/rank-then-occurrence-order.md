# rank-then-occurrence-order

- The comparator in `crates/retrieval/src/lexical/retrieve.rs` orders by raw
  rank, then occurrence identifier bytes; `better` keeps the lowest rank and,
  among equal ranks, the lowest probe ordinal.
- `crates/retrieval/tests/lexical_retrieval.rs:610`, `:651`, and `:1919`
  compare against an independent ordered reference.
