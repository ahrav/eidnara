# il-doc-read-nofollow-final-component-flat-list

## Discovery trigger

Issue #1105 Part A lens pass (security boundaries): the document reader's symlink
defense covers the final path component only, which is sufficient while every
name sits directly under the project root.

## Evidence trail

- `crates/daemon/src/project_docs.rs:9` `PROJECT_DOC_FILES` holds
  `ARCHITECTURE.md` and `STRUCTURE.md`, neither with a separator.
- `read_project_docs_canonical` (`project_docs.rs:174`) reads each name at
  `dir.join(filename)` (`:178`).
- `read_document` (`project_docs.rs:113-133`) calls `read_bounded_bytes`
  (`crates/daemon/src/config.rs:688`), which opens with `RDONLY | NOFOLLOW |
  NONBLOCK | CLOEXEC` and checks `is_file` on the descriptor
  (`config.rs:703-719`) and caps the bytes read.
- The `NotFound` arm classifies `Missing` when `std::fs::metadata(parent)` is a
  directory (`project_docs.rs:117-122`); `metadata` follows symlinks.
- `plan_document` retires the live row on `Missing`
  (`crates/daemon/src/source_capture.rs:1070-1076`).
- The hash covers `file:{name}\n{canonical}` (`project_docs.rs:208-211`).
- Tests: `project_docs.rs:244`, `:267`, `:304`, `:318`, `:387`, `:398`, `:429`;
  `crates/daemon/src/source_capture_tests.rs:2315`.

## Failure scenario

Under configured nested names (U8): a repository commits `docs` as a symlink
to a directory outside the root. A missing `docs/ARCH.md` classifies `Missing`
through the followed parent, and capture retires the live row; a present one
is read from outside the root.

## Timing windows and dependencies

A parent swap between the check and the open; at HEAD the only parent is the
project root.

## What a test must construct

U8: a symlinked intermediate directory with and without a target file, and a
kernel without `openat2`.

## Investigation log

### Q: Is any HEAD name nested?

- Sources examined: `project_docs.rs:9`; every caller of `PROJECT_DOC_FILES`.
- Findings: no; the list is a fixed array of two bare names.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the beneath-root resolver have to cover the `Missing` classification?

- Sources examined: `project_docs.rs:117-122`; #1105 OQ15.
- Findings: yes; the classification reads the parent's followed metadata.
- Missing evidence: U8's design.
- Conclusion: unresolved, owned by U8 and OQ15.
