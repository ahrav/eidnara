# canonical-memory-byte-pressure-is-exercised

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
This working-tree evidence follows the [catalog scope][scope], not an exercise.

## Discovery trigger

The independent review finds that a row-or-byte marker permits either limiting
mechanism to remain unchallenged. K4 requires byte pressure separately from K3.

## Evidence trail

- [read.rs:23-26][caps] separates the 8192-row bound from the 8 MiB byte budget.
- [read.rs:182-224][cutoff] selects rows first and then limits cumulative stored
  decision payload bytes. Domain/kind and scope exclusion precede both caps.
- [slice/read.rs:140-158][sizes] queries `length(decision_payload)` at the
  requested snapshot. This is BLOB byte length, not rendered summary length.
- [kernel_routes.rs:2156-2195][test] asserts a nonempty truncated newest prefix;
  it does not independently assert exact byte usage or maximal fitting count.

## Failure scenario

A wrong reader includes excluded decisions in its byte prefix. Large unrelated
payloads exhaust 8 MiB and hide a small eligible target. A row-only fixture can
pass while never creating this situation.

## Timing windows and dependencies

Keep admitted visible candidates at or below 8192, so row truncation cannot
mask the byte path. Order by created_commit_seq descending and object_id
ascending, not SQL lexical iteration. Excluded payloads precede the target and
sum to more than 8 MiB; relevant payloads through that target fit within 8 MiB.
Use valid bounded decision fields, not corrupt or unadmitted noise.

## What a test must construct

Independently record candidate count, target rank, eligibility, and the actual
stored decision_payload lengths, including JSON encoding inside each BLOB.
Separately measure full response JSON if a serializer cap is being evaluated;
summary characters, raw input characters, BLOB bytes, and response bytes are
not interchangeable. The marker asserts setup before reader invocation and
does not require the correct optimized cap to fire. K3 and K4 must both fire.
Existing checks remain unaudited; no pressure fixture or campaign runs here.

## Investigation log

### Q: Which byte measurement isolates canonical payload-prefix pressure?

- Sources examined: [The payload query][sizes], [cutoff][cutoff], and
  [generic byte test][test].
- Findings: The canonical cutoff consumes stored BLOB lengths, while the
  generic test verifies prefix shape rather than a maximal-fit byte oracle.
- Missing evidence: An independently certified small-count byte-pressure
  fixture is not yet constructed or executed.
- Conclusion: The measurement boundary is resolved; fixture execution remains
  pending. Byte pressure cannot substitute for K3's separate row witness.

[scope]: ../catalog.md#scope-and-provenance
[caps]: ../../../../crates/daemon/src/kernel_routes/read.rs#L23-L26
[cutoff]: ../../../../crates/daemon/src/kernel_routes/read.rs#L182-L224
[sizes]: ../../../../crates/kernel/src/slice/read.rs#L140-L158
[test]: ../../../../crates/daemon/tests/kernel_routes.rs#L2156-L2195
