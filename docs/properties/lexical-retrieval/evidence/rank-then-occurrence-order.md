# rank-then-occurrence-order

- The comparator in `crates/retrieval/src/lexical/retrieve.rs` orders by raw
  rank, then occurrence identifier bytes; sorting with `best_first` lets
  `dedup_by` keep the lowest rank and, among equal ranks, the lowest probe
  ordinal.
- `crates/retrieval/tests/lexical_retrieval.rs:641`, `:682`, and `:1950`
  compare against an independent ordered reference.
- `crates/retrieval/tests/lexical_retrieval.rs:2470` gives each of 150 matches
  its own rank and tombstones rows at the front and around the scan bound; the
  scan keeps the first `scan_rows` live rows of the reference order and reports
  truncation only while a live row lies past the bound.
