# il-doc-read-nofollow-final-component-flat-list

## Discovery trigger

Issue #1105 Part A lens pass (security boundaries): the document reader's symlink
defense covers the final path component only, which is sufficient while every
name sits directly under the project root.

## Evidence trail

- At `41d889ea4`, `PROJECT_DOC_FILES` held `ARCHITECTURE.md` and
  `STRUCTURE.md`, neither with a separator, and `read_project_docs_canonical`
  read each name at `dir.join(filename)`.
- At `41d889ea4`, `read_document` called `read_bounded_bytes`, which opened
  with `RDONLY | NOFOLLOW | NONBLOCK | CLOEXEC`, checked `is_file` on the
  descriptor, and capped the bytes read.
- At `41d889ea4`, the `NotFound` arm classified `Missing` when
  `std::fs::metadata(parent)` was a directory; `metadata` follows symlinks.
- `plan_document` retires the live row on `Missing`
  (`crates/daemon/src/source_capture.rs:1067-1073`).
- The hash covers `file:{name}\n{canonical}` (`project_docs.rs:356-359`).
- Tests: `project_docs.rs:397`, `:423`, `:463`, `:477`, `:549`, `:560`, `:594`;
  `crates/daemon/src/source_capture_tests.rs:2322`.

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

- Sources examined: the old reader; every caller of `PROJECT_DOC_FILES`.
- Findings: no; the list is a fixed array of two bare names.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the beneath-root resolver have to cover the `Missing` classification?

- Sources examined: the old reader; #1105 OQ15.
- Findings: yes; the classification reads the parent's followed metadata.
- Missing evidence: U8's design.
- Conclusion: unresolved, owned by U8 and OQ15.

### Q: What does U8 change?

- Sources examined: `crates/daemon/src/project_docs.rs:65`, `:193-275`;
  `crates/daemon/src/config.rs:1300`; the U8 tests.
- Findings: the document list is the user-tier `project_docs.files` key,
  admitted entry by entry with the default kept on any refusal. Each read opens
  the parent and then the final component with `openat2` beneath the root, so a
  symlinked or absent parent is `Unreadable`, a final-component symlink is
  `NotRegular`, and a kernel without `openat2` makes every read `Unreadable`.
- Missing evidence: a mount crossing inside the root, which needs privileges a
  test host lacks.
- Conclusion: resolved with answer: the record is invalidated.
