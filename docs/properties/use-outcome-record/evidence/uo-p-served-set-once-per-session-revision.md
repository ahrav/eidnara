# uo-p-served-set-once-per-session-revision

## Discovery trigger

Spec #1145 C6.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1156 branch tip, the commit that carries this
revision.

- Key and digest: `crates/daemon/src/use_outcome_writer.rs:402-405`; the request
  digest binds the ids, the harness, and the role (`request_digest`
  (`crates/daemon/src/use_outcome_writer.rs:453`)).
- Call site: `record_use_outcome` (`crates/daemon/src/lib.rs:9721`) runs after
  each committed attempt in `run_transform`.
- The slug keeps `revision`; the key is the served-set digest (OQ3-12).

- Guarantee under test: For one (project digest, harness, session, served-set
  digest), at most one serve commit exists; a later fold at an equal digest
  under the same harness and session role, at any `known_as_of`, replays and
  writes nothing, while another harness or role at that key conflicts.
- Existing check:
  `a_hard_fold_records_its_frozen_composition_once_per_served_set`
  (`crates/daemon/src/use_outcome_writer_tests.rs:70`);
  `an_equal_digest_replays_and_another_harness_or_role_conflicts`
  (`crates/daemon/src/use_outcome_writer_tests.rs:131`);
  `an_equal_served_set_at_a_later_known_as_of_replays`
  (`crates/daemon/src/use_outcome_writer_tests.rs:448`);
  `a_discarded_attempt_leaves_no_serve_and_the_retry_records_one`
  (`crates/daemon/src/use_outcome_writer_tests.rs:314`);
  `the_real_failure_paths_count_their_outcomes_and_bound_the_wait`
  (`crates/daemon/src/use_outcome_writer_tests.rs:719`);
  `a_recorded_served_set_replays_while_another_writer_holds_the_writer`
  (`crates/daemon/src/use_outcome_writer_tests.rs:817`);
  `every_kernel_error_maps_to_a_closed_outcome`
  (`crates/daemon/src/use_outcome_writer_tests.rs:942`);
  `an_emergency_pass_that_reruns_records_one_serve`
  (`crates/daemon/src/use_outcome_writer_tests.rs:961`);
  `only_an_unready_kernel_leaves_the_pass_to_retry_its_serve`
  (`crates/daemon/src/use_outcome_writer_tests.rs:989`);
  `a_hard_fold_answers_while_another_writer_holds_the_kernel_writer`
  (`crates/daemon/src/use_outcome_writer_tests.rs:1076`);
  `a_fold_past_the_writes_in_flight_counts_a_deadline_and_a_later_fold_repairs_it`
  (`crates/daemon/src/use_outcome_writer_tests.rs:1112`); Kernel half:
  `duplicate_and_malformed_serves_are_refused_without_rows`
  (`crates/kernel/tests/kernel_use_outcome.rs:215`),
  `a_repeated_serve_intent_replays_and_a_rebound_key_conflicts`
  (`crates/kernel/tests/kernel_use_outcome.rs:755`). Status: unaudited.

## Failure scenario

A second record for one served set.

## Timing windows and dependencies

Within one pass across CAS retries.

## What a test must construct

#1155 transform-route tests.

Required faults and enabling state: Two HARD folds with no Kernel change
between; a CAS retry.

## Investigation log

### Q: Receipts last for the database incarnation and serving records for the retention window

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Receipts last for the database incarnation and serving records for
  the retention window, so a fold at a served set whose record expired replays
  with no live record; recorded as a gap beside G2 (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: The receipt key omits the harness while the record identity carries it, so one session id served under two harnesses at one served set is a lasting conflict; the design takes session ids as unique within a project (needs human input)

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The receipt key omits the harness while the record identity carries
  it, so one session id served under two harnesses at one served set is a
  lasting conflict; the design takes session ids as unique within a project
  (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: A deadline, an unready Kernel, or a lost fence leaves the serve for a later write at its digest

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A deadline, an unready Kernel, or a lost fence leaves the serve for
  a later write at its digest: a rerun in the same pass when the Kernel was not
  Ready at the hand-off and no write was handed off (a write handed to a Ready
  Kernel settles the pass whatever its outcome), or a later HARD fold that
  another input requests over the same served set, such as context pressure.
  The pass hands the write to a tracked blocking job and answers while the job
  waits for the Kernel writer
  (`a_hard_fold_answers_while_another_writer_holds_the_kernel_writer`); when
  every write slot is taken, the write is counted `deadline` and repaired the
  same way
  (`a_fold_past_the_writes_in_flight_counts_a_deadline_and_a_later_fold_repairs_it`).
  A session whose composition never folds HARD again at that set stays
  unrecorded. A conflict and a refusal are not repaired by a later fold.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: Does the detached write stand as the resolution of OQ3-5, which resolved the write as inline within the pass's remaining budget?

- Sources examined: the ticket's OQ3-5 and AC6, commit `6d15ee29c`, and
  `record_use_outcome`'s hand-off to `TaskAdmission::spawn_blocking`.
- Findings: the pass hands the write to a tracked blocking job and answers; the
  job's own 250 ms writer wait is the budget AC6 then names, and the measured
  wait the ticket asked for is recorded by
  `the_real_failure_paths_count_their_outcomes_and_bound_the_wait`. The
  repository records the move and no approval of it.
- Missing evidence: the owner's confirmation that the detached placement
  resolves OQ3-5.
- Conclusion: needs human input.
