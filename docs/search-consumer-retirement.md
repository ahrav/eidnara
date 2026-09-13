# Search consumer retirement

`SearchSelection::retire` runs only after verified durable selection. Bootstrap
schema 2 retains the predecessor's consumer binding and seed certificate outside
the predecessor database. It preserves the replacement's lifecycle intent,
capture, and private construction family for coordinator handoff.
Ownership-only reads also accept bootstrap schema 1. They recover its explicit
consumer binding without opening or migrating the old SQLite database. Active
opens require bootstrap schema 2 and the matching projection baseline.
Serving and retirement metadata require complete, decoded certificates with
matching bindings. Byte-prefix matching is confined to cleanup of a known-owned
partial construction copy; it does not authenticate a serving or retiring family.

Retirement reads a bounded, fenced kernel inventory through the selection's
fixed target. Its source census is global: every canonical source descriptor
revision created through that target, not just sources seen by the old consumer.
Its barrier census is consumer-specific and includes satisfied memberships.
Wholesale removal of the old family safely covers the source superset. Source
and evidence invalidations are bounded by that target. A missing joined authority
row is an error. Before the first receipt, the old consumer's retained prefix is
verified from commit shapes alone; no outbox payload is read.

The census has its own bounds, `RetirementBounds`, separate from the per-batch
persist limits. `object_registry` is append-only, so the census through a fixed
target grows with corpus history and never shrinks. A census over its bound
fails with `InventoryBound` before any cleanup, and a retry with the same bounds
fails the same way. Size the bounds for the whole corpus. The kernel checks the
bound over at most one row more than it admits before materializing anything.

The database lease and immutable seed pin must drain physically. Cancellation
does not release them. Cleanup removes the old SQLite family and immutable seed,
then checks for unknown residue. Lease sidecars remain to preserve writer epochs;
they contain no projection payload. A process killed while inspecting the old
database leaves a private scratch copy beside it; the storage crate removes that
copy with the family under the same exclusive lease. Any other residue prevents
certification.

One replacement-local transaction commits the old consumer/generation binding,
selected family, target, and every completed removal disposition. Recovery checks
that receipt against authority before replaying an uncertain acknowledgement.
Calling the pure receipt mutation attests completed wholesale removal; it cannot
inspect physical holders or files. The daemon verifies their cleanup before
calling it. Exact authority, count, and byte comparisons validate receipt replay.
Ordinary acknowledgement follows local transaction and connection-lock release.
Ordinary deregistration may return `ConsumerPending` when the kernel tip advances;
retirement never raises the certified target to bypass that refusal. No automatic
abandonment path exists.

The original lifecycle deadline and caller budget bound retries. Gate admission
checks construction, transaction, and physical-drain limits. These checks do not
approve production envelopes: approved resource evidence remains required. The
path does not declare `Current`, enable hooks, implement lexical search, or change
default-disable behavior. Frequency-triggered external lexical evidence remains
deferred under its owner's approval.

The process-cut tests kill and reap children before and after receipt COMMIT,
after local release, and around acknowledgement, then reopen twice. This proves
process-crash recovery for those cuts, not power-loss durability. A separate
lost-response test revokes admission after acknowledgement but before
deregistration, then readmits retirement while the old consumer is still
registered. It checks exact disposition replay without replacing the receipt.
