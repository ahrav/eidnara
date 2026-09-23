# mtr-method-records-cite-executed-check

## Discovery trigger
Parent specification Phase 5 acceptance: "each record cites an executed
check"; `docs/properties/METHOD.md` record schema.

## Evidence trail
- `docs/properties/METHOD.md` fixes the twelve fields and the five semantics.
- `crates/eval-core/tests/method_records.rs` parses `catalog.md`, checks field order, semantics, cited
  tests (`declares_test` walks back from `fn name(` to a `#[test]` attribute
  and refuses an `#[ignore`), evidence files, and the index table.
- The CI `test` path filter includes `docs/properties/evaluator/**` so a
  catalog edit runs the test.

## Failure scenario
A test named in `Exercised` is renamed or marked ignored; the record keeps
citing it and the catalog claims evidence that no longer runs.

## Timing windows and dependencies
None.

## What a test must construct
The committed catalog; the test resolves each citation against the tree.

## Investigation log
### Q: Where do the records live?
- Sources examined: parent #749 Q-MOD-3; `docs/properties/AGENTS.md`.
- Findings: `docs/properties/evaluator/` as one part with cross-links.
- Missing evidence: none.
- Conclusion: resolved with answer - this directory.
### Q: Does the test see whether a cited test is filtered out by `cfg`?
- Sources examined: `declares_test`.
- Findings: no; a `#![cfg(...)]` at the file head is not read. Every cited
  daemon test is behind `all(unix, feature = "test-support")`, which CI runs.
- Missing evidence: none.
- Conclusion: resolved with answer - documented limit.
