# sc-repodoc-only-verified-missing-retires

## Discovery trigger

Part B record on #1066 (comment 2), decision D12. At baseline
`read_safe_canonical` returned `None` for a missing, symlinked,
oversized, non-UTF-8, or unreadable file, so a producer could not tell a
removal from a transient failure. Mapping all of them to removal would
erase the revision trail. U10 (#1089) split the dispositions and named
this record.

## Evidence trail

`crates/daemon/src/project_docs.rs`:

- `DocumentRead` has `Text`, `Missing`, `NotRegular`, `Oversize`,
  `NotUtf8`, `Unreadable`.
- `read_document`: a `NotFound` read is `Missing` only when the parent is
  an existing directory; otherwise `Unreadable`. Another error is
  classified from `symlink_metadata`: non-regular is `NotRegular`, longer
  than `MAX_PROJECT_DOC_BYTES` (256 KiB) is `Oversize`, anything else is
  `Unreadable`.

`crates/daemon/src/source_capture.rs`:

- `CaptureJob::plan_document`: text above `MAX_SOURCE_UNIT_BYTES`
  (64 KiB) and `Oversize` skip as `documents_oversize`; `NotRegular`,
  `NotUtf8`, `Unreadable` skip with their own counts; only `Missing`
  with a live row yields `DocumentStep::Retire`; `Missing` with no live
  row skips as `documents_missing`.
- `CaptureJob::retire_document` is the only retirement in the document
  path; its receipt key is `source-document-retire:{digest(object_id)}`.
- A rename is a `Retire` at the old path and a `Publish` at the new one
  in one job.

Other retirement paths for the class: `retire_captured_sources` handles
only `Messages` and `RawToolSpans`; harness-capture expiry applies to
`harness_capture` evidence only, and document evidence is `canonical`.
This agent found no other production writer that retires a
`repository_documents` descriptor.

## Failure scenario

A `None` or error read mapped to `Missing`, so a permission error, a
symlink swap, a file past a cap, or an unmounted root retires the live
revision and loses the trail.

## Timing windows and dependencies

- A transient permission error is `Unreadable` and retires nothing.
- A vanished project root is `Unreadable`, because the parent does not
  exist.
- An editor that unlinks then writes can show a `NotFound` with the
  parent present; that read is a verified `Missing` and retires. The next
  composing pass republishes above the historical head, so the lineage
  gains revisions and loses no bytes.
- A mount point left as an empty directory reads as removal (recorded in
  PR #1103); remount republishes above the head.
- A file that grows past a cap keeps its old live revision, which no
  longer matches the file.

## What a test must construct

A live document, then each of: a symlink at the path, text past 64 KiB,
a file past 256 KiB, non-UTF-8 bytes, a mode-0 file (skipped as root);
after each pass, the per-disposition count and the unchanged live row.
Also a removal, a missing file with no live row, a rename, and a missing
parent directory.

## Investigation log

### Q: Is the baseline citation still valid?
- Sources examined: `project_docs.rs` at HEAD.
- Findings: `read_safe_canonical` (`project_docs.rs:49-53` at baseline)
  is gone; `read_document` replaces it.
- Missing evidence: none.
- Conclusion: resolved with answer; citation moved.

### Q: Should a document that grows past a cap keep its stale live revision?
- Sources examined: `plan_document`; ticket-1089 scope; AC17.
- Findings: the ticket specifies that `Oversize` retires nothing, and the
  code follows it. The live descriptor then names bytes the file no
  longer holds, until the file shrinks below the cap or is removed.
- Missing evidence: an owner statement that a stale live revision is the
  intended reading for a document past a cap.
- Conclusion: needs human input.
