# use-outcome-link-names-own-session-prior-serve

## Discovery trigger

Spec D10 and ticket #1162 (OQ4-12): once the P3 `use_outcomes` table
exists, a dispute names the serve of its subject that the reporting
session received last before filing, so an operator can count disputes as
harm signals per served session.

## Evidence trail

- The route reads the envelope's `session_id` before the transport strips
  it and carries it, the binding's project digest, and the harness in a
  `Reporter` (`crates/daemon/src/kernel_routes/commit.rs:1106`).
- A dispute goes through `record_disposition`, which builds a
  `DisputeReporter` and calls `Envelope::record_dispute_admission`
  (`commit.rs:792`; `crates/kernel/src/admission.rs:1015`).
- `reporting_serve` (`crates/kernel/src/use_outcome.rs:279`) selects the
  `use_outcome_decisions` row naming the subject, joined to `use_outcomes`
  of the same (project digest, harness, session), with
  `served_commit_seq` below the envelope's commit sequence, greatest first.
- `write_reported_admission` (`admission.rs:1734`) writes the id in the
  same INSERT as the row, inside the writer transaction.
- The column, its `ON DELETE RESTRICT` foreign key, its partial index, and
  the immutability trigger are #1154's (`crates/kernel/src/schema.rs:164`).

## Failure scenario

A lookup keyed by session alone would link another harness's serve; a
lookup without the strict bound could link a serve the dispute's snapshot
could not see; a lookup by served set rather than decision rows would miss
the subject inside a larger composition.

## Timing windows and dependencies

The lookup and the write share one writer transaction, so a concurrent
serve commits wholly before or after the dispute. The DDL requires
`served_commit_seq < created_commit_seq`, so a serve recorded in the
dispute's own commit or later cannot satisfy the strict bound.

## What a test must construct

Two serves of the subject in one session, a serve of another decision in
that session, a serve of the subject in another session, a session never
served the subject, and a serve recorded after the dispute. Both
`a_dispute_links_the_reporting_sessions_latest_prior_serve_of_its_subject`
(`crates/kernel/tests/kernel_use_outcome.rs:1165`) and
`a_dispute_row_records_its_reporter_and_links_the_sessions_latest_serve`
(`crates/daemon/tests/kernel_routes.rs:6610`) construct these.

## Investigation log

### Q: Should a retired serving record be linkable?
- Sources examined: `reporting_serve`; the P3 sweep's cited-row predicate
  in `crates/kernel/src/use_outcome.rs`.
- Findings: the lookup reads retired records too. A record a committed
  dispute cites while its subject is live is never retired by the sweep.
- Missing evidence: a product decision on linking a record past its
  retention window.
- Conclusion: needs human input.

### Q: Does the link survive the P3 sweep?
- Sources examined: route test `kernel_routes.rs:6610`.
- Findings: after `expire_use_outcomes` at a time past the record's
  retention, the sweep reports one cited record and the linked record
  stays live.
- Missing evidence: none.
- Conclusion: resolved with answer: yes.
