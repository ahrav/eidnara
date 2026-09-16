# apply-consumer-capability-strings-never-authorize-edits

## Discovery trigger

The RP2.7 specification's adapter capability gate section and the RP2.7.U5
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u4-context-edits` at
`342cd18e`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/tests/support/kernel_daemon.rs` binds with `consumer_capabilities` from `StartOptions`.
- `crates/daemon/src` reads `consumer_capabilities` nowhere; the gate reads `RouteScope::context_capabilities` only.

## Failure scenario

A plugin claims a class and receives an edit the host cannot account for.

## Timing windows and dependencies

None: the consumer strings are read at bind and the declaration is latched; neither changes within a route epoch.

## What a test must construct

A `pi` route bound with advertising consumer strings.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: the harness-side applied-identity witnesses (RP2.8.U5).
- Conclusion: resolved for the gate; enablement witnesses remain open.
