# sc-admission-fixed-class-pair-under-event-other

Abbreviations follow [`../catalog.md`](../catalog.md): `hs.rs`, `hs-tests`,
`sc.rs`, and `sc-tests` name the harness-source and source-capture modules
and their tests.

## Discovery trigger

Part A record on admission provenance. R12 and the event-kind stop condition
require that publication never promote captured text. Re-verified at HEAD
`5a842d998`; U9 added `repository_documents` to the fixed table.

## Evidence trail

- `crates/daemon/src/harness_sources.rs` `admission_rule`:
  `CanonicalClaims`/`PromotedMemory` inherit their decision's classes;
  `RawToolSpans` -> `(TrustedToolResult, ToolUntrustedOutput)`;
  `GitCommits` and `RepositoryDocuments` -> `(UntrustedRepoText, RepoUntrustedText)`;
  role `user` -> `(ExplicitUser, UserExplicit)`; otherwise
  `(ModelInference, AssistantInference)`.
- `ProvenanceRule::resolve` returns the fixed pair or reads
  `subject_admission` inside the envelope.
- `SourcePublisher::apply` calls `record_admission` with `candidate_id: None`,
  `approval_object_id: None`, `kind: EventKind::Other`, and the evidence id,
  inside the descriptor commit. `publish_batch` calls the same `apply`.
- `crates/kernel/src/admission.rs` `event_effect`: `EventKind::Other` returns
  `(current, None, Outcome::Deny)`, so maturity and disposition stay.
- Tests read at HEAD:
  - `crates/daemon/tests/harness_sources.rs`
    `mixed_sessions_create_pending_only_for_message_text`: each published unit's
    `Published.admission` equals the fixture's pair, and an `admission_decisions`
    join returns exactly that pair and the unit's evidence object.
  - `repository_documents_publish_untrusted_supersede_and_project` and
    `crates/daemon/tests/git_sources.rs`
    `selected_commits_are_retained_exactly_and_rebuild_without_the_repository`:
    repository pair.
  - `crates/kernel/tests/kernel_admission.rs`
    `an_inert_citation_does_not_consume_approval_capacity`: an `Other` admission
    on a fresh subject has `effective_maturity == Maturity::Candidate`.
  - `visible_reads_exclude_decisionless_objects_and_apply_maturity_rows`:
    a `Candidate` subject is hidden from AutoInject.
  - `crates/daemon/src/source_capture_tests.rs`
    `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen`:
    after capture and a kernel reopen, no captured object is visible to
    AutoInject and the decision count is unchanged.
  - `sc-tests`
    `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
    reads the tool pair from `admission_decisions` through the capture path.

## Failure scenario

A role mapping change records assistant text as `ExplicitUser`, or a publisher
passes a promoting event kind. Captured text then reaches automatic surfaces.
Falsifier: a `Published.admission` outside the table, or a maturity above
`Candidate` after publication.

## Timing windows and dependencies

The pair is resolved in the commit, so an admission change for an inherited
subject between preview and commit does not carry forward. Fixed pairs depend
on no state.

## What a test must construct

One unit per (class, role), published on a fresh subject, with the pair read
from `Published.admission` and from `admission_decisions`, the
`event_kind`, `candidate_id`, and `approval_object_id` columns read back, and
maturity read after a reopen. Everything except the column read-back exists.

## Investigation log

### Q: Why lower `Exercised` from `yes` to `partial`?
- Sources examined: `mixed_sessions_create_pending_only_for_message_text`
  (selects only `source_class`, `taint_class`, `object_id`); `grep` for
  `event_kind`, `candidate_id`, `maturity` in `hs-tests`, `sc-tests`,
  `git_sources.rs`.
- Findings: no daemon test reads the event kind or the null candidate and
  approval ids on a publication's admission row. Maturity is covered through
  the kernel unit and the AutoInject reopen test.
- Missing evidence: a direct read-back.
- Conclusion: resolved with answer: `partial`.

### Q: Do baseline citations hold?
- Findings: baseline `hs.rs:1119-1140` is now `admission_rule`; `hs.rs:733-734`
  is `SourcePublisher::apply`; `admission.rs:4140-4161` is `event_effect`;
  `hs-tests:1515` is `mixed_sessions_create_pending_only_for_message_text`.
  Baseline `kernel_admission.rs:2369` is
  `an_inert_citation_does_not_consume_approval_capacity` and `:2692` is
  `visible_reads_exclude_decisionless_objects_and_apply_maturity_rows`.
- Conclusion: resolved with answer.

### Q: What reachability applies?
- Sources examined: `claim_sources.rs` `publish_batch` call; capture gate.
- Findings: `EventKind::Other` is recorded on every `apply`, including
  default-production `ClaimMaterializer` batches; the harness-class pairs run
  in production only behind the consent key.
- Conclusion: resolved with answer: `default-production`, with the harness
  pairs noted as config-gated.
