# Execution-lifecycle surface

This summarizes separate prior read-only surface discovery supplied in the
task, not a new test run or portfolio evaluation. Anchors are rechecked in
`/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. The
[external-evidence scope](../catalog.md#scope-and-provenance) is pending final
confirmation; historical citations and exercise are not carried forward.

[Dispatch][dispatch] separates pending-settlement and handler-task permits.
The callback's tracker is distinct from the outer task's tracker membership.
[Close][close] waits for tracked work and refuses cleanup after an unquiesced
post-abort timeout. [The wire contract][wire] retains best-effort cancellation,
first-terminal arbitration, and unknown outcomes without an observed terminal.

[Ingest guards][ingest] demonstrate retained-byte ownership through blocking
work and result retention. Their lifetime is not the pending-table lifetime.
E1-E3 apply these preservation obligations to execution placement without
claiming that [the transform closure][transform] already runs off-worker.

The [current resource ledger][ledger] separates pending count, task count,
ingress bytes, parse residency, and staging/decode resources. Future queued,
running, and retained-result ownership needs explicit capacity decisions.
Worker-specific test readiness is BLOCKED; only current callbacks have established
production reachability. No new epoch, global duration, or exactly-one transform
effect is inferred. Existing fenced CAS and operation reconciliation remain
the basis for reasoning about a cancelled request's durable outcome.

[dispatch]: ../../../../crates/host-runtime/src/dispatch.rs#L823-L998
[close]: ../../../../crates/host-runtime/src/dispatch.rs#L1237-L1268
[wire]: ../../../host-wire-protocol.md#L742-L781
[ingest]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L513-L550
[transform]: ../../../../crates/daemon/src/lib.rs#L8182-L8254
[ledger]: ../evidence/request-work-accounting-covers-retained-resources.md#evidence-trail
