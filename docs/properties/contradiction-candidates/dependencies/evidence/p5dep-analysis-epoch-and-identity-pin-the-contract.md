# p5dep-analysis-epoch-and-identity-pin-the-contract

HEAD `3a3b916b7`. Surface S1.

## Discovery trigger

`crates/retrieval/AGENTS.md`: "A change to any analysis or compilation rule in
`src/lexical/` needs a new `ANALYSIS_CONTRACT_EPOCH` and an updated pinned
digest in `tests/lexical_analysis.rs`." Plan P5 U3 wants derived rows that
are "rebuildable with identical results"; the epoch is the natural version
to record with them. Lens: version compatibility; D4 in `system-model.md`.

## Evidence trail

- `crates/retrieval/src/lexical/mod.rs:39-40`: `ANALYSIS_CONTRACT_EPOCH:
  &str = "identifier-analysis.v1"`, "Changes whenever an analysis or
  compilation rule changes without changing any other identity component."
- `mod.rs:4-5`: `AnalysisIdentity` "changes whenever the contract epoch, the
  `char::UNICODE_VERSION` of the toolchain, the tokenizer, the detail mode,
  the indexed columns, or the linked SQLite version change."
- `crates/retrieval/src/lexical/identity.rs:13-18`: `CURRENT: LazyLock`
  computes SHA-256 of `preimage()` once per process.
- `identity.rs:34-65` `preimage()`: fields in fixed order `epoch`,
  `ANALYSIS_CONTRACT_EPOCH`, `unicode`, `{major}.{minor}.{update}`,
  `tokenizer`, `TOKENIZER`, `detail`, `DETAIL`, `columns`, `ORIGINAL_COLUMN`,
  `PARTS_COLUMN`, `sqlite`, `rusqlite::version()`, `sqlite_source_id`,
  `SQLITE_SOURCE_ID`; each written as `"{len}:{field}\n"`. Doc at `:35`:
  "The Unicode version is included because atom and part boundaries come
  from the toolchain's `char` classification tables; the SQLite version
  because `unicode61` folds with the engine's own tables."
- `tests/lexical_analysis.rs:389-412` `identity_pins_the_manifest`: asserts
  each field appears in the preimage, `ANALYSIS_CONTRACT_EPOCH ==
  "identifier-analysis.v1"`, and `AnalysisIdentity::current().as_str() ==
  "b624de2e...ba83"` with the message "a changed manifest field changes the
  identity; update the contract and digest together".
- `tests/lexical_analysis.rs:1`: goldens "authored by hand from the
  documented rules rather than from analyzer output" (the hand-written
  digest requirement in the task statement refers to this discipline plus
  the pinned literal).
- Working tree: `rust-toolchain.toml` modified to add the `rust-analyzer`
  component; channel stays `1.98`, so `UNICODE_VERSION` is unchanged.

## Failure scenario

A toolchain bump to a Rust release with a newer Unicode table changes
`char::is_alphanumeric` for newly assigned code points, changing atom
boundaries for texts containing them. The epoch string stays
`identifier-analysis.v1`; the identity digest changes and the pinned test
fails, forcing a deliberate update. A P5 consumer that stored only the epoch
with its candidate rows would consider rows built under the old toolchain
current; a consumer that stored the identity digest would see them as stale
and rebuild. Conversely, a rule edit without an epoch bump still changes
goldens but not the digest; the goldens catch it.

## Timing windows and dependencies

Build time only. `AnalysisIdentity::current()` is constant per process.

## What a test must construct

- The existing pinned test is the check. A campaign-level check would build
  under two toolchains and assert the digests differ; this is out of tree.
- For P5: a test that the derived table's recorded identity equals
  `AnalysisIdentity::current()` at write time (handoff to the P5 domain
  model).

## Investigation log

### Q: Should P5 rows record the identity digest rather than the epoch?

- Sources examined: `identity.rs`, `mod.rs:4-5`, `AGENTS.md`, plan U3.
- Findings: the identity is strictly finer than the epoch and is the value
  the search projection uses to detect drift (`mod.rs:4`). The plan names
  only the analyzer, not the version to record.
- Missing evidence: the P5 author's intent.
- Conclusion: needs human input.
