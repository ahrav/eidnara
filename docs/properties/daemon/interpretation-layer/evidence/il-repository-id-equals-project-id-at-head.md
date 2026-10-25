# il-repository-id-equals-project-id-at-head

## Discovery trigger

Issue #1105 Part A lens pass (state and persistence): document capture writes the
project digest into both identity fields.

## Evidence trail

- `crates/daemon/src/source_capture.rs:1028-1080` `plan_document`; the identity
  vector at `:1048-1052` holds `("project_id", P)`, `("repository_id", P)`, and
  `("relative_path", name)`.
- `:1053` derives the lineage from that identity; `:1055-1057` reads the live row
  and keeps it only for `CAPTURE_DOMAIN_ID` (`source_capture.rs:51`).
- A missing document retires the live row (`:1058-1064`) through
  `retire_document` (`:1083-1105`), called from `capture_documents` at `:937`.
- `crates/kernel/tests/eval_identity.rs` pins the identity encoding, not the
  values.

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

### Q: Can a cap on the history walk flip the derived identity?

- Sources examined: #1105 OQ1 and U7's plan text.
- Findings: U7's stability scenario fixes the history; no record covers a cap
  near the root count.
- Missing evidence: the cap value.
- Conclusion: unresolved, needs OQ1's decision.

### Q: What does U7 change?

- Sources examined: `crates/daemon/src/repository_identity.rs:62`;
  `crates/daemon/src/source_capture.rs:1011`, `:998`, `:1050`, `:1426`; the U7 tests.
- Findings: the identity's `repository_id` is `git:` and the smallest root
  commit reachable from HEAD, or the project digest as a deterministic
  fallback, memoized per project for the incarnation. A pass retires every
  live capture-domain row of a settled path under another `repository_id`
  before it publishes.
- Missing evidence: none.
- Conclusion: resolved with answer: the record is invalidated.
