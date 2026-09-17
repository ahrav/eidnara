# apply-adapter-validates-entire-assembled-invocation

## Discovery trigger

The RP2.7 specification's adapter capability gate section and the RP2.7.U5
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u4-context-edits` at
`342cd18e`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/edit_receipts.rs` capacity check before minting.
- The adapter half is RP2.8.U5.

## Failure scenario

An edit that fits its own bound overflows the invocation.

## Timing windows and dependencies

None: the invocation bound is checked on one assembled request.

## What a test must construct

The RP2.8.U5 adapter.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: the harness-side applied-identity witnesses (RP2.8.U5).
- Conclusion: resolved for the gate; enablement witnesses remain open.
