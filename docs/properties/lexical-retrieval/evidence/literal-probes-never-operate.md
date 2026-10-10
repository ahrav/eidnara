# literal-probes-never-operate

- `crates/retrieval/src/lexical/compile.rs` `compile` quotes each atom with
  `quote_atom`, doubling any `"`.
- `crates/retrieval/tests/lexical_engine.rs:196` and `:213` run the probes on a
  populated FTS5 engine against hand-written term sets, and show that the
  unquoted control lets `OR` and `NEAR` operate.
- `crates/retrieval/tests/lexical_retrieval.rs:551` runs request text holding
  operators through `retrieve` on a projection.
- `crates/retrieval/tests/lexical_analysis.rs:236` shows analysis never yields
  a quote inside an atom.
