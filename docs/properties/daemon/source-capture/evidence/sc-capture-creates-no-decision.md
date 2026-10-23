# sc-capture-creates-no-decision

## Discovery trigger

Part B obligation from #1066 comment 2, requirements R12 and R13, decisions
D4 and D14. At baseline the CI "Invented provenance" gate scanned no capture
path. The U2 ticket (#1082) adds the AE6 reopen test and extends the gate; the
U10 ticket (#1089) adds repository documents.

## Evidence trail

`crates/daemon/src/harness_sources.rs`:

- `SourcePublisher::apply` publishes the descriptor and calls
  `record_admission` with `EventKind::Other`, `candidate_id: None`, and
  `approval_object_id: None`. It creates no memory decision and sets no
  `decision_kind`.
- `SourcePublisher::evidence_request` sets `provenance` to
  `crate::git_sources::commit_provenance(unit.unit)`.

`crates/daemon/src/git_sources.rs` - `commit_provenance` returns `None` for
every class other than `OccurrenceClass::GitCommits`, including
`RepositoryDocuments`, `Messages`, and `RawToolSpans`.

`crates/daemon/src/source_capture.rs` - `CaptureJob::publish` builds the
capture publisher with `sensitivity: Sensitivity::Sensitive` and
`egress: ProviderEgress::LocalOnly`.

`crates/kernel/src/cas/ingest.rs` - ingest folds an inspected payload with
`asserted_sensitivity == Normal` and no affirmative provenance to
`Sensitive`; capture asserts `Sensitive` directly.

`.github/workflows/ci.yml`, step "Invented provenance" - the path set now
includes `crates/daemon/src/harness_sources.rs`,
`crates/daemon/src/source_capture*`, and `crates/daemon/src/project_docs.rs`.
The only permitted populations are `provenance: None` and the one line
forwarding `commit_provenance(unit.unit)`.

`crates/kernel/src/admission.rs` - `surface_visibility` decides surface
visibility by class pair and sensitivity.

## Failure scenario

A `RepositoryDocuments` arm reuses the git provenance path, or an asserted
`Normal` is honored. Captured bytes then become model-visible under a weaker
policy, or a capture appears as a memory decision.

## Timing windows and dependencies

None. Kernel reopen is the state that must preserve the property.

## What a test must construct

Publish user text and tool results through the transform path, count the
decisions before, drop the handler, reopen the Kernel store from disk, and
assert no captured object is visible to `Surface::AutoInject` and the decision
count is unchanged. For repository documents, assert the admission pair
`(UntrustedRepoText, RepoUntrustedText)`, `Sensitive`, and an unchanged
decision count.

Landed checks:

- `crates/daemon/src/source_capture_tests.rs`
  `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen`
  constructs the reopen case.
- `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
  asserts `(TrustedToolResult, ToolUntrustedOutput)` and `Sensitive` for a
  `read` output (AE3).
- `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`
  asserts the document admission pair, `Sensitive`, and no new decision.
- The CI "Invented provenance" step covers the capture modules.

## Investigation log

### Q: Is the absence of `RepositoryProvenance` asserted on a stored row?

- Sources examined: `source_capture_tests.rs` (no `provenance` match),
  `PROGRESS.md` U2 deferrals ("TS7 provenance-null assertion").
- Findings: no landed test reads a captured evidence row's provenance. The
  property rests on `commit_provenance` and the CI grep.
- Missing evidence: a row-level assertion that captured evidence carries no
  provenance, for messages, tool spans, and documents.
- Conclusion: unresolved, needs that assertion; U2 deferred it.

### Q: Is `EventKind::Other` asserted on capture admissions?

- Sources examined: the U2 tests' `admission_decisions` queries.
- Findings: the queries read `source_class` and `taint_class`, not the event
  kind. `EventKind::Other` is a literal in `SourcePublisher::apply`; the
  Part A record `sc-admission-fixed-class-pair-under-event-other` carries it.
- Missing evidence: none for this record.
- Conclusion: resolved with answer.
