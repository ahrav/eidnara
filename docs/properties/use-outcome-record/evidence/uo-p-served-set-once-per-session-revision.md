# uo-p-served-set-once-per-session-revision

## Discovery trigger

Spec #1145 C6.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `d5ebb3598`).

- Key and digest: `crates/daemon/src/use_outcome_writer.rs:89`; the request
  digest binds the ids, the harness, and the role (`request_digest`
  (`crates/daemon/src/use_outcome_writer.rs:134`)).
- Call site: `record_use_outcome` (`crates/daemon/src/lib.rs:9628`) runs after
  each committed attempt in `run_transform`.
- The slug keeps `revision`; the key is the served-set digest (OQ3-12).

- Guarantee under test: For one (project digest, harness, session, served-set
  digest), at most one serve commit exists; a later fold at an equal digest, at
  any `known_as_of`, replays and writes nothing.
- Existing check:
  `a_hard_fold_records_its_frozen_composition_once_per_served_set`
  (`crates/daemon/src/use_outcome_writer_tests.rs:60`);
  `an_equal_digest_replays_and_another_harness_or_role_conflicts`
  (`crates/daemon/src/use_outcome_writer_tests.rs:121`);
  `an_equal_served_set_at_a_later_known_as_of_replays`
  (`crates/daemon/src/use_outcome_writer_tests.rs:359`);
  `a_discarded_attempt_leaves_no_serve_and_the_retry_records_one`
  (`crates/daemon/src/use_outcome_writer_tests.rs:280`). Status: unaudited.

## Failure scenario

A second record for one served set.

## Timing windows and dependencies

Within one pass across CAS retries.

## What a test must construct

#1155 transform-route tests.

Required faults and enabling state: Two HARD folds with no Kernel change
between; a CAS retry.

## Investigation log

### Q: OQ3-10 (gap G2)

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-10 (gap G2): a session that continues after its delete replays
  its earlier receipt at an equal composition; accepted as a loss at #1155
  ticketing.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
