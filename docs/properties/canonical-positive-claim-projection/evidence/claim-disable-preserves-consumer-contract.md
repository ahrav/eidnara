# claim-disable-preserves-consumer-contract

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/kernel/tests/kernel_outbox.rs`; `crates/daemon/tests/search_replacement/recovery.rs` - `explicit_recovery_bootstraps_deregistered_and_pending_disabled_consumers`.

## Failure scenario

Disabling releases retention for history a consumer never applied.

## Timing windows and dependencies

None.

## What a test must construct

A lagging consumer at disable time.

## Investigation log

### Q: What is the legal pending-consumer transition for the default-deregister plan?

- Sources examined: the checks in the evidence trail and the specification text.
- Findings: see the evidence trail.
- Missing evidence: the construction named above where it is not yet in the tree.
- Conclusion: needs human input
