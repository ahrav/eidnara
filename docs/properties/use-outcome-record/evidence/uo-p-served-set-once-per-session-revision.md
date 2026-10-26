# uo-p-served-set-once-per-session-revision

## Discovery trigger

Spec #1145 C6.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `dc07f442a`).

- Key and digest: `crates/daemon/src/use_outcome_writer.rs:284`; the request
  digest binds the ids, the harness, and the role (`length_prefixed_digest`
  (`crates/daemon/src/use_outcome_writer.rs:339`)).
- Call site: `record_use_outcome` (`crates/daemon/src/lib.rs:9684`) runs after
  each committed attempt in `run_transform`.
- The slug keeps `revision`; the key is the served-set digest (OQ3-12).

- Guarantee under test: For one (project digest, harness, session, served-set
  digest), at most one serve commit exists; a later fold at an equal digest, at
  any `known_as_of`, replays and writes nothing.
- Existing check:
  `a_hard_fold_records_its_frozen_composition_once_per_served_set`
  (`crates/daemon/src/use_outcome_writer_tests.rs:64`);
  `an_equal_digest_replays_and_another_harness_or_role_conflicts`
  (`crates/daemon/src/use_outcome_writer_tests.rs:125`);
  `an_equal_served_set_at_a_later_known_as_of_replays`
  (`crates/daemon/src/use_outcome_writer_tests.rs:429`);
  `a_discarded_attempt_leaves_no_serve_and_the_retry_records_one`
  (`crates/daemon/src/use_outcome_writer_tests.rs:324`);
  `the_real_failure_paths_count_their_outcomes_and_bound_the_wait`
  (`crates/daemon/src/use_outcome_writer_tests.rs:591`);
  `every_kernel_error_maps_to_a_closed_outcome`
  (`crates/daemon/src/use_outcome_writer_tests.rs:689`);
  `an_emergency_pass_that_reruns_records_one_serve`
  (`crates/daemon/src/use_outcome_writer_tests.rs:708`);
  `only_an_unready_kernel_leaves_the_pass_to_retry_its_serve`
  (`crates/daemon/src/use_outcome_writer_tests.rs:736`); Kernel half:
  `duplicate_and_malformed_serves_are_refused_without_rows`
  (`crates/kernel/tests/kernel_use_outcome.rs:214`),
  `a_repeated_serve_intent_replays_and_a_rebound_key_conflicts`
  (`crates/kernel/tests/kernel_use_outcome.rs:694`). Status: unaudited.

## Failure scenario

A second record for one served set.

## Timing windows and dependencies

Within one pass across CAS retries.

## What a test must construct

#1155 transform-route tests.

Required faults and enabling state: Two HARD folds with no Kernel change
between; a CAS retry.

## Investigation log

### Q: Receipts last for the database incarnation and serving records for the

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Receipts last for the database incarnation and serving records for
  the retention window, so a fold at a served set whose record expired replays
  with no live record; recorded as a gap beside G2 (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: The receipt key omits the harness while the record identity carries it

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The receipt key omits the harness while the record identity carries
  it, so one session id served under two harnesses at one served set is a
  lasting conflict; the design takes session ids as unique within a project
  (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: A deadline, an unready Kernel, or a lost fence leaves the serve for a 

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A deadline, an unready Kernel, or a lost fence leaves the serve for
  a later write at its digest: a rerun in the same pass when the Kernel was not
  Ready and no write was attempted (every commit outcome settles the pass, so a
  pass waits on the writer at most once), or a later HARD fold that another
  input requests over the same served set, such as context pressure. A session
  whose composition never folds HARD again at that set stays unrecorded. A
  conflict and a refusal are not repaired by a later fold.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
