# p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S2. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`retire_evidence` (`crates/kernel/src/slice/write.rs:585`) selects the row with the `EVIDENCE_CITED_SQL` predicate and returns `Conflict` when cited (`:607`). `delete_artifact` (`crates/kernel/src/cas/deletion.rs:239`) runs `UPDATE evidence_meta SET invalidated_commit_seq=...` for every live row of the digest (`:389`) with no citation predicate. `validate_trigger` (`crates/kernel/src/admission.rs:2807`) then fails for an observation backed by that row. `evidence_cited_in_refuses_a_deleted_evidence_row_that_live_decisions_still_cite` (`crates/kernel/tests/kernel_slice.rs:2248`) deletes a cited artifact and asserts the evidence is invalidated while its citing decision stays live.

## Failure scenario

A deleted artifact removes the evidence an attestation's witness rested on.

## Timing windows and dependencies

A deletion between the verifier's read and its commit.

## What a test must construct

A decision citing evidence, then `delete_artifact` on its digest.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Should `delete_artifact` honor the citation guard?

- Sources examined: `delete_artifact` and `retire_evidence`.
- Findings: The two paths differ; the specification records the difference without choosing.
- Missing evidence: see the conclusion.
- Conclusion: needs human input
