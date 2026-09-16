# apply-suppression-requires-confirmed-surviving-span

## Discovery trigger

The RP2.7 specification's adapter capability gate section and the RP2.7.U5
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u4-context-edits` at
`342cd18e`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/edit_receipts.rs` `unconfirmed_survivor` and the `Action::Suppress` branch of `ReceiptStore::prepare`.
- `docs/host-wire-protocol.md` Section 7.8 fixes the `survivors` field and the five reasons.

## Failure scenario

A span the harness no longer shows is suppressed as though visible.

## Timing windows and dependencies

A backend answer that changes during a route epoch.

## What a test must construct

A harness declaring suppression; survivor sets of each shape.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: the harness-side applied-identity witnesses (RP2.8.U5).
- Conclusion: resolved for the gate; enablement witnesses remain open.
