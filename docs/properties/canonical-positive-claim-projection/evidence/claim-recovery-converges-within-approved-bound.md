# claim-recovery-converges-within-approved-bound

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/search_replacement/recovery.rs`, `crates/daemon/tests/search_catchup.rs` process-crash harness.

## Failure scenario

Recovery that never converges leaves stale candidates authoritative.

## Timing windows and dependencies

Recovery after each crash cut.

## What a test must construct

An approved RP2.9 recovery bound; a crash at each boundary.

## Investigation log

### Q: Which numeric recovery bound does RP2.9 approve for claim projections?

- Sources examined: the checks in the evidence trail and the specification text.
- Findings: see the evidence trail.
- Missing evidence: the construction named above where it is not yet in the tree.
- Conclusion: needs human input
