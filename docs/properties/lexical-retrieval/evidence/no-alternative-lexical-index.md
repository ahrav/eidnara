# no-alternative-lexical-index

- `crates/retrieval/baseline.sql:94` defines the one `lexical` FTS5 table with
  the `unicode61` tokenizer and `detail = full`.
- `crates/retrieval/tests/schema_inventory.rs:312` compares the baseline with
  the frozen inventory field for field.
- `crates/retrieval/tests/lexical_engine.rs:248` shows no prefix expansion.
