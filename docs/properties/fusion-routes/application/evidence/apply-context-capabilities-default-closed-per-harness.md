# apply-context-capabilities-default-closed-per-harness

## Discovery trigger

The RP2.7 specification's adapter capability gate section and the RP2.7.U5
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u4-context-edits` at
`342cd18e`; inspected 2026-09-16.

## Evidence trail

- `crates/host-runtime/src/model_execution/backend.rs`: `context_capabilities` defaults to `ContextCapabilities::NONE`; `OPENCODE_CONTEXT_CAPABILITIES` and `PI_CONTEXT_CAPABILITIES`; `HarnessDispatchBackend` forwards per harness; `opencode.rs` and `pi.rs` answer their own harness.
- `crates/daemon/src/context_capabilities.rs`: `CapabilitySource`, `LatchedCapabilities::read` and `gate`, `CapabilityDenial`.
- `crates/daemon/src/lib.rs`: `bind` latches into `SessionBinding::context_capabilities`; `RouteScope` carries it; `edit_receipts.rs` gates `retrieval.prepare`.
- `crates/daemon/src/bin/eidnara_host/serve.rs` installs `BackendDeclarations` over the production backend; it reads each harness's `unavailable_reason` and `context_capabilities` once at construction, because the backends' availability read revalidates the installed closure.

## Failure scenario

A harness adapter that cannot construct or account for an edit is offered one.

## Timing windows and dependencies

A backend answer that changes during a route epoch.

## What a test must construct

A mutable source, the recorded tables, and a daemon with no source.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: the harness-side applied-identity witnesses (RP2.8.U5).
- Conclusion: resolved for the gate; enablement witnesses remain open.
