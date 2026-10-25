# il-repository-id-equals-project-id-at-head

## Discovery trigger

#1105 Part A lens pass (state and persistence): document capture writes the
project digest into both identity fields.

## Evidence trail

- `crates/daemon/src/source_capture.rs:951-1002` `plan_document`; the identity
  vector at `:970-974` holds `("project_id", P)`, `("repository_id", P)`, and
  `("relative_path", name)`.
- `:975` derives the lineage from that identity; `:977-979` reads the live row
  and keeps it only for `CAPTURE_DOMAIN_ID` (`source_capture.rs:48`).
- A missing document retires the live row (`:980-986`) through
  `retire_document` (`:1005-1022`), called from `capture_documents` at `:908`.
- `crates/kernel/tests/eval_identity.rs` pins the identity encoding, not the
  values.
- The document tests (`crates/daemon/src/source_capture_tests.rs:1964`,
  `:2137`, `:2202`, `:2276`) read rows through `live_document`, which selects
  by `relative_path` alone (`source_capture_tests.rs:1903-1909`). The
  `project_id` and `repository_id` values at `source_capture_tests.rs:2164-2167`
  build another producer's fixture; no test asserts the values capture writes.

## Failure scenario

Two worktrees of one repository bind two project digests. Each captures
`ARCHITECTURE.md` under its own `(P, P, path)` lineage; the two lineages never
correlate, so a reader cannot tell they describe one repository's file.

## Timing windows and dependencies

None at HEAD. After U7, the derived `repository_id` depends on repository
history, and a pass must retire a live row whose `repository_id` differs.

## What a test must construct

A capture test with two project roots that share a repository and an
assertion over both lineages' identities; the U7 tests replace it.

## Investigation log

### Q: Does any HEAD code derive `repository_id` from the repository?
- Sources examined: `source_capture.rs`, `project_docs.rs`, the capture
  configuration.
- Findings: no; the value is the bound project digest.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the document tests assert the identity fields?
- Sources examined: `source_capture_tests.rs:1903-1909`, `:1964`, `:2137`,
  `:2164-2167`, `:2202`, `:2276`.
- Findings: no. They assert revisions, retention, retirement, and counts;
  the identity values they hold belong to a foreign fixture.
- Missing evidence: none.
- Conclusion: resolved with answer: corrected in the record's `Exercised`
  line.

### Q: Can a cap on the history walk flip the derived identity?
- Sources examined: #1105 OQ1 and U7's plan text.
- Findings: U7's stability scenario fixes the history; no record covers a cap
  near the root count.
- Missing evidence: the cap value.
- Conclusion: unresolved, needs OQ1's decision.
