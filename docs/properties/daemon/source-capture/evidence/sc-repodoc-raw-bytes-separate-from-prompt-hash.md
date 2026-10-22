# sc-repodoc-raw-bytes-separate-from-prompt-hash

## Discovery trigger

Part B record on #1066 (comment 2), decision D12. The prompt block hashes
canonical document content (BOM stripped, CRLF folded, trailing
whitespace trimmed), so a capture keyed on that hash would miss
whitespace edits and could not reproduce the file the agent read. U10
(#1089) landed the raw read beside the canonical read and named this
record.

## Evidence trail

`crates/daemon/src/project_docs.rs`:

- `read_document` returns `DocumentRead::Text(String)` holding the bytes
  `config::read_bounded_bytes` read, decoded as UTF-8 with no rewrite.
- `read_project_docs_canonical` builds `canonical_hash` and
  `rendered_block` from `canonicalize_doc_content(raw)` and keeps the raw
  reads in `ProjectDocs.reads`, so the hash and the goldens are
  unchanged.
- `leave_reads` moves the reads into the per-pass `DocumentReads` slot;
  `clear_reads` empties it for a discarded composition.

`crates/daemon/src/source_capture.rs`:

- `SourceCapture::offer` takes the slot's reads at handoff.
- `CaptureJob::plan_document` compares the live row's
  `detail.artifact_digest` with `payload_id(text.as_bytes())` and skips
  as `documents_unchanged` on equality. It never reads the canonical
  hash. Other text becomes a `SourceUnit` whose `text` is the raw read.

Compose sites: `m0_compose::compose_m0` and
`transform::compose_additive_m0` call `leave_reads`;
`apply_once_with_estimator` empties the slot at the top of each CAS-retry
attempt (per PR #1103; this agent located the `leave_reads` and
`clear_reads` helpers and did not re-read both call sites).

## Failure scenario

The skip compares the canonical hash, so a whitespace-only edit is never
captured. Or capture re-reads the file after the response, so the
retained bytes differ from what the prompt was built from. Or the
artifact stores canonical text, so the evidence cannot reproduce the
file.

## Timing windows and dependencies

- Swap or growth after open: the bounded no-follow read validates the
  open descriptor and enforces the cap on bytes read, so the raw bytes
  are the bytes the canonical form was built from in the same pass.
- A pass that reuses its prompt block reads no documents and captures
  none; an edit waits for the next composing pass.
- A CAS-retry attempt empties the slot, so only the attempt that
  produced the response leaves reads.
- A document whose text the redactor flags is refused at publication
  (Part A `sc-unprovenanced-text-stores-sensitive-or-refuses-secrets`);
  then nothing is retained and the guarantee holds vacuously.

## What a test must construct

A document captured at revision 1; a reuse pass after an edit; an
unchanged file; an edit; a whitespace-only edit with the canonical hash
compared before and after; reads of the artifact bytes after each
publication.

## Investigation log

### Q: Does the landed test compare a pass's own `docs_hash`?
- Sources examined:
  `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`;
  PR #1103 "Recorded and left as written".
- Findings: the test compares `read_project_docs_canonical(...)
  .canonical_hash` before and after the whitespace edit, not the pass's
  stored `docs_hash`. The canonical block and the capture share one read,
  so the two values come from the same function.
- Missing evidence: a pass-level `docs_hash` comparison.
- Conclusion: resolved with answer; the gap is recorded as a
  refinement.

### Q: Do the baseline citations still hold?
- Sources examined: `project_docs.rs` at HEAD.
- Findings: the baseline cited `read_safe_canonical`
  (`project_docs.rs:45-53`). At HEAD that function no longer exists;
  `read_document` replaces it and returns `DocumentRead`.
  `canonicalize_doc_content` is still present.
- Missing evidence: none.
- Conclusion: resolved with answer; citation moved.
