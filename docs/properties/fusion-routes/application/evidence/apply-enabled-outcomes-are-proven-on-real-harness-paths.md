# apply-enabled-outcomes-are-proven-on-real-harness-paths

## Discovery trigger

The RP2.7 specification's adapter capability gate section and the RP2.7.U5
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u4-context-edits` at
`342cd18e`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/tests/context_capabilities.rs` drives `opencode` and `pi` routes under the recorded tables.
- The applied-identity witnesses need the RP2.8.U5 harness-side apply.

## Failure scenario

A class is declared enabled without a harness proving it.

## Timing windows and dependencies

A backend answer that changes during a route epoch.

## What a test must construct

A running harness with the RP2.8.U5 apply.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: the harness-side applied-identity witnesses (RP2.8.U5).
- Conclusion: resolved for the gate; enablement witnesses remain open.
