# use-outcome-link-names-own-session-prior-serve

## Discovery trigger

Spec D10 and ticket #1162 (OQ4-12): once the P3 `use_outcomes` table
exists, a dispute names the serve of its subject that the reporting
session received last before filing, so an operator can count disputes as
harm signals per served session.

## Evidence trail

- The route reads the envelope's `session_id` before the transport strips
  it and carries it, the binding's project digest, and the harness in a
  `DisputeReporter` (`crates/daemon/src/kernel_routes/commit.rs:1104`, built
  at `:1167`).
- A dispute goes through `record_disposition`, which builds a
  `DisputeReporter` and calls `Envelope::record_dispute_admission`
  (`commit.rs:790`; `crates/kernel/src/admission.rs:1015`).
- `reporting_serve` (`crates/kernel/src/use_outcome.rs:299`) runs
  `REPORTING_SERVE_SQL` (`use_outcome.rs:184`): it seeks the
  `use_outcome_sessions` row of (project digest, harness, session), then
  seeks `idx_use_outcome_decisions_object_fk` at (subject, `session_seq`)
  for the greatest `served_commit_seq` below the envelope's commit
  sequence, ties to the greatest `use_outcome_id`. Each decision row
  copies its record's `session_seq` and `served_commit_seq`, which the
  `use_outcome_decisions_written_with_record` trigger checks
  (`crates/kernel/src/schema.rs:198`), and a record names its session
  through a foreign key over `session_seq` and its identity columns
  (`schema.rs:194`), so the seek answers exactly the record join it
  replaces. The plan is pinned by
  `the_sweep_and_the_reads_use_their_indexes`
  (`use_outcome.rs:1100`): two index seeks, no sort, whatever the number
  of serves in the session or of the subject.
- `write_reported_admission` (`admission.rs:1737`) writes the id in the
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
serve commits wholly before or after the dispute, and the link is written
once, so a serve recorded after the dispute leaves it as it was. The DDL
requires `served_commit_seq < created_commit_seq`, so a serve recorded in
the dispute's own commit would satisfy the strict bound; the route never
writes one, because `apply` handles observations, decisions, supersedes,
retirements, and dispositions and no other operation
(`crates/daemon/src/kernel_routes/commit.rs:917-1060`), and the serve
writer records in its own commit (`crates/daemon/src/use_outcome_writer.rs:449`).

## What a test must construct

Two serves of the subject in one session, a serve of another decision in
that session, a serve of the subject in another session, a session never
served the subject, and a serve recorded after the dispute. Both
`a_dispute_links_the_reporting_sessions_latest_prior_serve_of_its_subject`
(`crates/kernel/tests/kernel_use_outcome.rs:1371`) and
`a_dispute_row_records_its_reporter_and_links_the_sessions_latest_serve`
(`crates/daemon/tests/kernel_routes.rs:6651`) construct these.
`a_session_is_recorded_once_and_its_decision_rows_copy_their_record`
(`crates/kernel/tests/kernel_use_outcome.rs:1610`) checks that a session is
recorded once, that another session, harness, or project gets its own
`session_seq`, and that every decision row carries its record's
`session_seq` and `served_commit_seq`.
`a_tie_on_the_served_sequence_links_the_greater_record_id`
(`crates/kernel/tests/kernel_use_outcome.rs:1658`) builds two records of one
session served at the same sequence.
`the_stored_link_equals_the_record_join_definition`
(`crates/kernel/tests/kernel_use_outcome.rs:1694`) generates serve histories
with shared and out-of-order served sequences, overlapping served sets, two
harnesses, and a session never served, and checks that every dispute stores
the record the `use_outcome_decisions` and `use_outcomes` join answers under
the ticket's definition.

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
- Sources examined: route test `kernel_routes.rs:6651`.
- Findings: after `expire_use_outcomes` at a time past the record's
  retention, the sweep reports one cited record and the linked record
  stays live.
- Missing evidence: none.
- Conclusion: resolved with answer: yes.
