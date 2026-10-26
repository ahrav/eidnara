# uo-p-served-set-once-per-session-revision

## Discovery trigger

Spec #1145 C6.

Record kind: proposed obligation; its owning ticket #1155 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `8ace6c9f8`).

- Slug kept; the guarantee names the served-set digest (OQ3-12).

- Guarantee under test: For one (project digest, harness, session, served-set
  digest), at most one serve commit exists; a later fold at an equal digest
  replays and writes nothing.
- Existing check: Kernel half only:
  `duplicate_and_malformed_serves_are_refused_without_rows`
  (`crates/kernel/tests/kernel_use_outcome.rs:215`);
  `a_repeated_serve_intent_replays_and_a_rebound_key_conflicts`
  (`crates/kernel/tests/kernel_use_outcome.rs:731`). The writer is proposed.
  Status: unaudited.

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
