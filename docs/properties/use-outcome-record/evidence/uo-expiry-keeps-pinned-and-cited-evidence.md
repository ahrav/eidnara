# uo-expiry-keeps-pinned-and-cited-evidence

## Discovery trigger

Spec #1145 K2.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `03c6b2b95`).

- Pin predicate: `crates/kernel/src/local_file.rs:514`.
- Citation predicate: `crates/kernel/src/slice/write.rs:42`.
- The use-outcome citation predicate keys liveness on the citing row's subject
  the same way (`crates/kernel/src/use_outcome.rs:149`).

- Guarantee under test: Capture expiry keeps evidence a live hold pins and
  evidence another live row cites.
- Existing check:
  `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`
  (`crates/daemon/tests/harness_sources.rs:3482`);
  `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence`
  (`crates/kernel/tests/kernel_source_descriptors.rs:1895`);
  `a_capture_pinned_after_it_was_selected_for_expiry_is_kept`
  (`crates/kernel/src/local_file.rs:751`). Status: unaudited.

## Failure scenario

Evidence retired under a live citation would leave the citing row dangling.

## Timing windows and dependencies

Between selection and the retiring commit.

## What a test must construct

The existing tests.

Required faults and enabling state: An expired capture cited by a live
observation; one pinned by a live hold.

## Investigation log

### Q: Gap G5

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Gap G5: every retention class a writer stores is a compiled
  constant? `harness_capture`, `curator_capture`, and `use_outcome_capture` are
  constants; `canonical` has no Rust constant (spec correction at #1154
  ticketing). Unresolved, needs an owner decision on a `canonical` constant
  (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.
