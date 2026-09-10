# export-fixed-s-exactly-once

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases P/I/N/R/L/D
resolve in [catalog sources](../catalog.md#sources).

## Discovery trigger

The data-integrity pass separates fixed-S exactness from paging size and
retention. P, line 82, requires a stable `(class, object_id, revision)` cursor.
P, lines 132-135, requires insert/delete interleavings across multiple pages
and equality with snapshot S exactly once. These are claims under test.

## Evidence trail

- `crates/kernel/src/slice/read.rs:64-71` opens one deferred transaction per
  `slice_as_of` call and commits after loading the whole slice.
- `crates/kernel/src/slice/read.rs:177-191` checks negative and future S.
  It does not bind multiple calls to an export cursor or retention witness.
- `crates/kernel/src/slice/read.rs:194-215` selects decisions created by S
  and not invalidated at S, ordered by decision ID, then collects them.
- `crates/kernel/src/slice/read.rs:298-341` applies the corresponding rule
  to observations. This is not an all-class export.
- `crates/kernel/src/envelope.rs:686-711` distinguishes live-only objects
  from history whose invalidation and supersession fields are masked after S.
- `crates/kernel/src/envelope.rs:395-438` updates `domains.name` in place and
  emits `operator_remediation` with the unchanged source revision. Its only
  supported target is a domain name (`:240-242`), not every projected class.
- `crates/kernel/tests/kernel_envelope.rs:246-289` checks that masking around
  correction and retirement. Status: unaudited; no run is claimed.
- `crates/kernel/tests/kernel_slice.rs:466-559` checks selected live IDs at
  different snapshots, not page composition at one fixed S.
- `crates/kernel/tests/kernel_proofs/obligations/o5_correction.rs:234-278`
  freezes snapshots before each correction and checks their stability. The
  operation model at `crates/kernel/tests/kernel_proofs/model.rs:95-120`
  offers a reference-set seam limited to three object kinds, not all export bytes.
- `crates/retrieval` is absent and daemon source has no export consumer.
  The new property is `test-only` because production paged export is absent.

## Failure scenario

A bootstrap reads the first page at S. A correction commits before the second
page. If the second query uses the current tip, or resumes by an unstable
offset, the export can contain both the old and replacement row, neither row,
or future invalidation metadata. All individual page reads may look valid.

A count-only oracle can hide this: one duplicate and one omission cancel.
Using the same exporter twice as actual and expected can repeat the same bug.
The oracle must compare identity, revision, required history facts, and bytes.
If approved source mapping consumes a remediated domain name, key/revision
equality alone cannot recover its old bytes at S. Export must supply valid
required S input or abort; it cannot restore removed canonical plaintext or
invent a revision change. No projected dependency is established by this path.

## Timing windows and dependencies

The vulnerable window is between successive page transactions. S must remain
constant while the underlying tip advances. The retention record separately
requires the old bytes/history to remain available or the attempt to abort.
The export must cross both key and class boundaries, including an empty end.
The source-coverage owner defines which canonical histories are export inputs.

## What a test must construct

1. Seed canonical operations in an independent ledger and capture S only
   after the bootstrap fence is durably registered.
2. Force several small pages, with at least one unread and one read key.
3. Correct/delete those keys and insert another key after the first page.
4. Check every cursor advance and that all accepted pages retain S.
5. Compare the concatenated complete result to sorted `E(S)` with exact
   multiplicity, not just set membership or a row count.
6. Record `search_projection_export_writes_between_pages` from operation timing, independently
   of the equality assertion. The marker fires even when export is correct.
7. Commit domain-name remediation during export and account for its control
   event. Record `search_projection_export_operator_remediation_during_snapshot`; compare
   affected bytes only if approved mapping includes them, otherwise record
   nondependence. No new occurrence generation is prescribed.

## Investigation log

### Q: Can an existing snapshot reader already satisfy the page contract?

- Sources examined: P current-code map and KTD2; slice/read and envelope above.
- Findings: Historical predicates and metadata masking exist; readers collect
  whole vectors. The proposed cursor is not present.
- Missing evidence: Bounded multi-page production reader and interleaving run.
- Conclusion: Resolved with answer: reuse predicates/decoding, not a nonexistent
  paginated API. The guarantee remains unexercised.

### Q: Which rows and tombstones define the independent export oracle?

- Sources examined: P R1, KTD2, U1/U5; envelope live/history queries.
- Findings: P requires class/revision/tombstone equality but does not specify
  the all-class export representation. Live-only and history queries differ.
- Missing evidence: Agreed source-coverage mapping and stable class ordering.
- Conclusion: Needs human input from the source-coverage/kernel owners.

### Q: Does domain-name remediation establish a projected dependency?

- Sources examined: `crates/kernel/src/envelope.rs:240-242`, `:395-438`; P R1.
- Findings: Only domain names are supported; the operation changes bytes in
  place without advancing source revision. RP2.1 field mapping is undefined.
- Missing evidence: Approved mapping consuming that field and valid-S/abort
  observation. The analyst's unconditional projected-dependency claim is too broad.
- Conclusion: Needs human input. Conditionally reuse
  [projection-remediation-invalidates-derived-bytes](../../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes)
  when the mapping consumes it; otherwise do not manufacture that dependency.
