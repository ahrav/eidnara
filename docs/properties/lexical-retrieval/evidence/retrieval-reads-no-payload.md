# retrieval-reads-no-payload

- `crates/retrieval/AGENTS.md` limits `lexical::retrieve` to `lexical`,
  `occurrences`, and `occurrence_tombstones`.
- `crates/retrieval/tests/lexical_retrieval.rs:1244` renames `payloads` away
  and asserts the same retrieval result.
