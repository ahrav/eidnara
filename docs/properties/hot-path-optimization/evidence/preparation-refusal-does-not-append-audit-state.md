# preparation-refusal-does-not-append-audit-state

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Moving values into an audit structure earlier can make a refused preparation
leave metadata behind even when no output value is returned.

## Evidence trail

- [memory-store/lib.rs:465][bound] sets the durable text limit to 512 KiB.
- [2204-2243][prepare] checks input first, refuses detected NewIdentity next,
  checks chosen output, and appends a scan only after all three checks.
- [2245-2271][execute] persists audit only for an applied operation in its
  fenced transaction; a callback error propagates rather than applying audit.
- [8921-8931][refusal] turns transaction preparation failure into an error
  because returning a successful Replay disposition would commit earlier writes.
- [22694-22725][test] constructs a fitting input whose redacted output exceeds
  the bound, then a replacement-adjusted exactly fitting output.

## Failure scenario

A refactor records the scan before checking replacement expansion or secret
identity refusal. The failed field appears in the prepared audit sequence.
If transaction failure is misclassified as a successful disposition, earlier
effects may also commit despite the refusal.

## Timing windows and dependencies

The in-memory append boundary and durable commit boundary are separate.
Earlier successful preparations can remain in the local PreparedWrite; refusal
must leave that sequence unchanged, not necessarily empty. A failed enclosing
write must leave neither its effects nor its audit delta committed.

## What a test must construct

Snapshot scans after an earlier successful field, then attempt each refusal:
input above 512 KiB, output expansion above the limit, and detected NewIdentity.
Repeat relevant cases across both scan layers. Compare the scan sequence before
and after, then compare durable effect/audit projections on enclosing failure.
The bound and rollback tests are [unaudited][checks]; no new experiment runs.

## Investigation log

### Q: Does a durable rollback test establish no in-memory scan append?

- Sources examined: [Preparation ordering][prepare], [execution][execute], and
  [the replacement-growth test][test].
- Findings: Durable rollback can hide an early local append. The growth test
  checks refusal and a fitting output but does not inspect the scan sequence.
- Missing evidence: A before/after scan observation and its relation to the
  enclosing transaction failure are not supplied.
- Conclusion: The two observation boundaries must remain separate; the
  in-memory witness is unresolved and must be added by the test handoff.

[bound]: ../../../../crates/memory-store/src/lib.rs#L550
[prepare]: ../../../../crates/memory-store/src/lib.rs#L2289-L2328
[execute]: ../../../../crates/memory-store/src/lib.rs#L2330-L2356
[refusal]: ../../../../crates/memory-store/src/lib.rs#L9223-L9233
[test]: ../../../../crates/memory-store/src/lib.rs#L23204-L23235
[checks]: ../existing-checks.md#redaction-ownership
