# uo-review-reads-authorize-by-bearer-and-project-binding-not-owner

## Discovery trigger

Spec #1145 K5 and lead L3.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip, the commit that carries this
revision.

- Scope: `kernel_route_scope` (`crates/daemon/src/kernel_routes/mod.rs:414`).
- Correction: `memory_reviewer_wire.rs:409` tests `review.accept`, not reads.

- Guarantee under test: `review.*` reads authorize by the connection bearer key
  and the route's project binding; the wire has no owner principal, and an
  observational `cli` route reads the same answers.
- Existing check:
  `review_operations_are_disabled_without_module_authority_and_list_and_read_receipts_under_it`
  (`crates/daemon/tests/memory_reviewer_wire.rs:15`);
  `an_observational_route_reads_the_same_outcomes_as_an_ordinary_route`
  (`crates/daemon/tests/memory_reviewer_wire.rs:260`);
  `review_use_outcome_counts_serves_and_later_disputes_of_one_decision`
  (`crates/daemon/tests/memory_reviewer_wire.rs:686`). Status: unaudited.

## Failure scenario

An observational route refused would hide the counts from the CLI.

## Timing windows and dependencies

None.

## What a test must construct

Marker `uo_observational_route_read_review_outcome`, owner #1157.

Required faults and enabling state: An observational route and an ordinary route
bound to one project.

## Investigation log

### Q: Lead L3

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Lead L3: the spec's "owner-only" read has no wire principal;
  restated in C17.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
