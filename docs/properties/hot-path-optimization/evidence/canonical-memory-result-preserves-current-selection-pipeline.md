# canonical-memory-result-preserves-current-selection-pipeline

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The supplied canonical-read surface audit proposes reducing candidate work.
The preservation target is the selected result, not a particular SQL shape.

## Evidence trail

- [daemon/lib.rs:8190-8270][pass] pins one read before the synchronous transform.
  Reruns at [8264, 8291, 8317, and 8372][reruns] reuse that captured result.
- [canonical_memory.rs:141-174][entry] captures tip before lag sampling and
  passes that tip to the auto-inject read; failures become withheld outcomes.
- [admission.rs:3485-3699][query] builds the served query over admission
  decisions, folds own/lineage authority and sensitivity in Rust, and collects
  rows. [admission.rs:3193][order] sorts the collected rows by object ID.
- [read.rs:163-258][selection] applies domain/kind and scope selection, keeps
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
[unaudited](../existing-checks.md#canonical-read). This record is partially
exercised: `the_domain_selection_equals_the_filtered_full_read` in
`crates/kernel/tests/kernel_canonical_read.rs` compares the pushed-down read
with the full read filtered in Rust, and the `admission::tests` fold tests pin
the Rust history fold to the SQL it replaced. The comparison against the fixed
baseline commit's rendered bytes does not run yet.

## Investigation log

### Q: Can excluded-row decoding be skipped without preserving its errors?

- Sources examined: [The query][query] and [post-query selection][selection].
- Findings: Decoding and collection precede caller exclusion, so a corrupt
  excluded row can affect the baseline outcome before filtering.
- Missing evidence: The proposed pushdown and an owner decision for these errors
  are not supplied. The fixed reference identity does not approve suppression.
- Later finding (#443): the owner approved suppression. The memory read's
  query matches `o.domain_id=:domain_id AND o.object_kind='decision'` before
  any row is decoded, so a corrupt row outside the domain no longer withholds
  memory; a read that serves that row still decodes it and fails on it.
- Conclusion: Error observability was an owner gate before M1 pushdown; the owner decision closes it.
  Oracle packaging as a test-only adapter or reference executable is a test-design
  choice, not an open semantic acceptance rule; candidate helpers are not oracles.

[pass]: ../../../../crates/daemon/src/lib.rs#L8201-L8281
[reruns]: ../../../../crates/daemon/src/lib.rs#L8348-L8452
[entry]: ../../../../crates/daemon/src/canonical_memory.rs#L141-L174
[query]: ../../../../crates/kernel/src/admission.rs#L3485-L3699
[order]: ../../../../crates/kernel/src/admission.rs#L3193
[selection]: ../../../../crates/daemon/src/kernel_routes/read.rs#L163-L258
[conversion]: ../../../../crates/daemon/src/canonical_memory.rs#L184-L212
