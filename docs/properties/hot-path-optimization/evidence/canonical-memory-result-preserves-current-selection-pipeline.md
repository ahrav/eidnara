# canonical-memory-result-preserves-current-selection-pipeline

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The supplied canonical-read surface audit proposes reducing candidate work.
The preservation target is the selected result, not a particular SQL shape.

## Evidence trail

- [daemon/lib.rs:8133-8202][pass] pins one read before the synchronous transform.
  Reruns at [8264, 8291, 8317, and 8372][reruns] reuse that captured result.
- [canonical_memory.rs:141-174][entry] captures tip before lag sampling and
  passes that tip to the auto-inject read; failures become withheld outcomes.
- [admission.rs:3132-3298][query] joins admission decisions, folds own/lineage
  authority and sensitivity, and collects rows. SQL order is object ID order.
- [read.rs:159-248][selection] applies domain/kind and scope selection, keeps
  newest rows, checks typed-row completeness, and trims the payload prefix.
- [canonical_memory.rs:184-212][conversion] applies Visible and positive-category
  filters only after those caps, then trims memory and creates the snapshot.

## Failure scenario

A pushdown moves a category filter before a cap and retains a different row.
Alternatively, it drops uncertain scopes or skips a sensitivity restriction.
The final block, revision, or withholding state then changes under optimization.

## Timing windows and dependencies

The reference and candidate need identical governing snapshots and lag inputs.
Serving rank is `created_commit_seq DESC, object_id ASC`, not query iteration
order. The canonical path uses the read's payload-prefix bound; it does not
gain permission to replace it with a different serializer-budget algorithm.

## What a test must construct

Compare ordered IDs/content, exact rendered bytes, revision, known-as-of,
truncation, and outcome against commit
`913234433ae36a80a6e22c6aac14c7f9aab74386`. Its admission query, read_visible,
and canonical conversion are the fixed semantic baseline. Include ties,
negative categories, Labeled candidates, uncertain scope terms, admission and
lineage restrictions, and both row and payload boundaries. Existing checks are
[unaudited](../existing-checks.md#canonical-read); this record is not exercised.

## Investigation log

### Q: Can excluded-row decoding be skipped without preserving its errors?

- Sources examined: [The query][query] and [post-query selection][selection].
- Findings: Decoding and collection precede caller exclusion, so a corrupt
  excluded row can affect the baseline outcome before filtering.
- Missing evidence: The proposed pushdown and an owner decision for these errors
  are not supplied. The fixed reference identity does not approve suppression.
- Conclusion: Error observability remains an owner gate before M1 pushdown.
  Oracle packaging as a test-only adapter or reference executable is a test-design
  choice, not an open semantic acceptance rule; candidate helpers are not oracles.

[pass]: ../../../../crates/daemon/src/lib.rs#L8133-L8202
[reruns]: ../../../../crates/daemon/src/lib.rs#L8264-L8375
[entry]: ../../../../crates/daemon/src/canonical_memory.rs#L141-L174
[query]: ../../../../crates/kernel/src/admission.rs#L3132-L3298
[selection]: ../../../../crates/daemon/src/kernel_routes/read.rs#L159-L248
[conversion]: ../../../../crates/daemon/src/canonical_memory.rs#L184-L212
