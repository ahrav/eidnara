# projection-source-inventory-complete

## Discovery trigger

The protocol and product lenses find five required source classes in RP2.1 R1.
U4 requires comparison by identities, bytes and coverage after ingestion,
revision and deletion. A shared count can hide a missing class, so the oracle
must identify each source occurrence independently of the projector.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Evidence consists of source inspection and documented claims, not executions.

- [RP2.1 requirements][requirements] name messages, canonical claims, promoted
  memory, git commits and selected raw tool spans in one local schema.
- [U4][plan] names durable rows, supervisor sweeps and per-class falsifiers.
- [N1.3 inventory][n1] assigns raw message projection, git durable rows/sweeps,
  promoted-memory embedding and shared timer slices to RP2.1.
- [Canonical memory reader][memory] filters memory-domain visible decisions
  and trims to a rendering budget. It cannot enumerate every source class.
- `crates/daemon/src/canonical_memory.rs:282-312` is the filtering test, not
  the reader. Its existing assertion remains unaudited adjacent evidence.
- [OpenCode tool decoding][tool] expands one native tool part into a call and,
  for completion/error, a result block. Native record identity is not block count.
- [Workspace][workspace] has no retrieval crate. Named search rows and source
  inventory are absent; daemon routes reject message/git projection methods.

Reachability: `test-only`. The current codecs and canonical memory reader are
reachable, but no production five-class durable projection or message/git
source adapter implements this inventory. Those adjacent paths do not justify
labelling the proposed completeness check production-reachable.

## Failure scenario

Message and claim rows are ingested, but a git sweep or promoted-memory source
is never connected. A total row count still rises. A faulty completeness report
uses those same observed rows as its denominator and reports full coverage.
The missing class becomes invisible to both implementation and checker.

A competing explanation is a legitimate source-policy exclusion. Selected raw
spans are not every byte as an independently ranked row, and a retired source
can remain in historical metadata. The expected inventory must therefore bind
policy, class, source revision, representation, span and deletion meaning.
It includes an explicit excluded population rather than treating every omission
as valid after the fact. No new claim maturity policy is introduced here.

## Timing windows and dependencies

Compare at a declared complete checkpoint, not while a legal batch is pending.
The export owner supplies a consistent canonical prefix and fenced rebuild
oracle. This record consumes that oracle; it does not recreate export logic.
Revision and deletion must be compared within the same source incarnation.
For immutable git commits, exercise source retirement or selection removal,
not a fabricated in-place mutation of the Git commit object.
Both harnesses matter for message and tool adapters; canonical sources need
their own class checks rather than an artificial harness encoding.

## What a test must construct

1. A fixture-owned expected inventory for all five classes, including a class
   with zero selected rows and its known source denominator.
2. At least one accepted example per class and each class's supported revision
   or deletion transition, with exclusion reasons defined before projection.
3. A promoted memory linked to its canonical source and a distinct claim
   representation that shares text without collapsing occurrence identity.
4. Native tool records whose call/result expansion would make block counts
   disagree with source occurrence counts.
5. Interrupted class batches followed by a declared catch-up boundary.
6. Per-class comparison of identities, tombstones and multiplicities, not just
   aggregate counts or sets that silently collapse duplicate rows.
7. Separate class precondition markers from [fault-map](../fault-map.md).
8. An actual completeness declaration while independent bounded `E(c,p)` is
   known and nonempty. Merely offering class inputs does not reach the safety
   check's antecedent. Equality is checked separately from this witness.

No five-class test exists. Existing codec and canonical-reader checks are
unaudited adjacent coverage. Their exact guarantees are reused by link in
[existing-checks](../existing-checks.md#overlap-register).

## Investigation log

### Q: What are the canonical mappings and source-selection policies?

- Sources examined: [requirements][requirements], [U4][plan], [N1.3][n1],
  canonical memory and tool decoding.
- Findings: Required classes and ownership are explicit; existing rendered
  memory is only a subset and cannot define promoted-memory/search membership.
- Missing evidence: Canonical message/git adapters, promotion identity, source
  retirement mapping and selected-span policy.
- Conclusion: needs human input from projection and source-adapter owners.

### Q: What bound prevents indefinite incomplete coverage?

- Sources examined: RP2.1 lines 108-121, 153-156 and acceptance gates.
- Findings: Catch-up and sweeps are required; production values are unset.
- Missing evidence: Approved finite fault-free catch-up/sweep bound and units.
- Conclusion: the [normal catch-up and authorized recovery record][progress]
  owns finite progress, with numeric approvals from RP2.9. Its `Current`
  witnesses feed the binding acceptance matrix. Completeness safety alone
  does not certify that the consumer makes progress.

[requirements]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L35-L41
[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L151-L156
[n1]: ../../../../../../commons/docs/plans/2026-09-08-1614-feat-eidnara-rust-product-state-ownership-plan.md#L156-L169
[memory]: ../../../../../crates/daemon/src/canonical_memory.rs#L141-L212
[tool]: ../../../../../crates/daemon/src/codec/opencode.rs#L490-L559
[workspace]: ../../../../../Cargo.toml#L3-L17
[progress]: ../../export-recovery/catalog.md#rp21-catchup-and-authorized-recovery-converge
