# cc-pass-and-sweep-write-no-kernel-event-and-no-signal

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

Candidates are lexical overlaps; they must never change Kernel state or reserve
reviewer work.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/pass.rs` reads through
  `capture_identified_commit_read_target_within_budget`,
  `visible_domain_decisions_as_of_in_scope`, `scope_terms_within_budget`,
  `claim_standing_at_within_budget`, `decision_payload_sizes_as_of`, and
  `decisions_for_objects_as_of_within_budget`, checks `require_incarnation`
  before each write, and writes only
  `replace_candidates_for_descriptor_bounded` on the memory store.
- `crates/daemon/src/contradiction_candidates/sweep.rs` reads and deletes
  through `delete_candidates_as_read` only.

## Failure scenario

A pass that filed a dispute or a reviewer job would turn a lexical overlap into
authority.

## Timing windows and dependencies

None; structural.

## What a test must construct

Tip and job-fact comparisons around every direct pass and sweep, a handler pass,
and the lexical source checks in
`crates/daemon/tests/contradiction_candidates_sweep.rs`.

## Investigation log

### Q: Can the source checks see indirect calls?

- Sources examined: the evidence trail above.
- Findings: no; they are lexical over the module's files.
- Missing evidence: none.
- Conclusion: the record stays `partial`.
