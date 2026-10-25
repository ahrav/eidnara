# sc-project-docs-canonical-hash-over-fixed-list

## Discovery trigger

Part A record of the source-capture catalog (#1066 comment 1), written at
baseline `08de9051a`: the prompt's project-docs block hashes a canonical form,
so no raw-byte revision trail exists (R7). U10 (`ticket-1089.md`) captures raw
bytes beside this read and lists this record as exercised with goldens
unchanged. This file re-verifies it at HEAD `5a842d998`.

## Evidence trail

`crates/daemon/src/project_docs.rs`:

- `PROJECT_DOC_FILES` is `["ARCHITECTURE.md", "STRUCTURE.md"]`;
  `MAX_PROJECT_DOC_BYTES` is 256 KiB.
- `canonicalize_doc_content` strips a leading BOM, maps CRLF to LF, trims
  trailing spaces and tabs per line, and trims trailing newlines.
- `read_document` reads through `config::read_bounded_bytes` and returns
  `DocumentRead::{Text, Missing, NotRegular, Oversize, NotUtf8, Unreadable}`.
  `Missing` requires the parent to exist as a directory.
- `read_project_docs_canonical` reads both files in order, hashes
  `file:{name}\n{canonical}` pieces joined by `PROJECT_DOCS_DELIMITER` with
  SHA-256, XML-escapes the rendered block, and returns the raw reads in
  `ProjectDocs::reads`.
- `leave_reads` moves the reads into the pass's `DocumentReads` sink.

`crates/daemon/src/config.rs`:

- `read_bounded_file` opens with `NOFOLLOW | NONBLOCK | CLOEXEC`, checks the
  descriptor is a regular file, and enforces the cap on bytes read.
- `inject_docs` defaults `true`; `ConfigKey::MemoryClassifierInjectDocs`
  (`/memory_classifier/inject_docs`) is `TierClass::UserOnly`.

`crates/daemon/src/m0_compose.rs`: the composer calls
`read_project_docs_canonical` when `inputs.inject_docs`, passes the reads to
`leave_reads`, renders `docs.rendered_block` into m0, and stores
`docs.canonical_hash` in `M0Composition::docs_hash`.

Tests read at HEAD (`project_docs.rs` tests module):
`symlinked_doc_is_skipped`, `a_doc_that_outgrows_the_cap_under_the_read_is_skipped`
(sparse oversize and FIFO), `a_file_is_missing_only_from_a_directory_that_exists`
(added by U10), `oversized_doc_is_skipped`, and
`project_docs_golden_matches_reference`. The golden
`crates/daemon/testdata/project-docs-golden.json` has six cases; case 3 holds a
BOM, trailing spaces, a CRLF, a tab, and trailing newlines. Its git history
shows only the initial import commit, so U10 left it unchanged.

`crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`
writes a whitespace-only edit and asserts the canonical hash is unchanged while
the captured artifact holds the raw edited bytes.
`document_capture_leaves_the_response_and_respects_inject_docs` asserts equal
responses with capture on and off and no capture when `inject_docs` is false.

## Failure scenario

A whitespace-only edit changes the hash, evicting the cached m0 prefix. A
symlink, FIFO, oversize, or non-UTF-8 file reaches the prompt. A third file
name enters the block.

## Timing windows and dependencies

The open is no-follow on the final component and the descriptor type is
checked after the open, so a swap of the file itself after the open cannot
change what is read. A swap of a parent directory component before the open is
outside `NOFOLLOW`. The module doc comment still says the window is narrowed,
not eliminated.

## What a test must construct

Whitespace-only edits of each kind against the golden; F18 fixtures (symlink,
FIFO, sparse oversize, non-UTF-8) beside one regular file; `inject_docs: false`.

## Investigation log

### Q: Is `docs_hash` persisted, as `history_segment_coverage.rs` says?

- Sources examined: grep for `docs_hash` and `docsHash` across `crates/`,
  `packages/`, SQL, and Markdown at HEAD.
- Findings: the only hits are the field and its doc in `m0_compose.rs`, its
  assignment from `canonical_hash`, and two doc lines in
  `history_segment_coverage.rs` that say it is persisted with the rendered m0
  bytes. No code reads or persists the field.
- Missing evidence: none for the code side.
- Conclusion: resolved with answer: no reader or writer exists; the comment
  disagrees with the code (reported to the lead).

### Q: Do the baseline citations still hold?

- Sources examined: `project_docs.rs`, `config.rs`, `m0_compose.rs` at HEAD.
- Findings: `project_docs.rs` grew the `DocumentRead` enum, `read_document`,
  `leave_reads`, and `clear_reads` in U10; the test lines moved and are cited by
  name. `config.rs:682-712` is now `read_bounded_bytes` at `:688` and
  `read_bounded_file` from `:696`. `m0_compose.rs:39-41` still holds
  `docs_hash`; the assignment is at `:254`.
- Missing evidence: none.
- Conclusion: resolved with answer.
