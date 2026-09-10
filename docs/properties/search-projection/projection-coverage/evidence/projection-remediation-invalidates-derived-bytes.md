# projection-remediation-invalidates-derived-bytes

## Discovery trigger

Independent analyst `ses_f7623dcccffe3Y09nW2wVoABif` raised G1. The user supplied
the finding and its qualification: a canonical field changes without source
revision changing, but actual RP2.1 dependence on that field is not established.
This is a conditional existing-authority claim, not a demonstrated projection
defect. The disposition does not prescribe a new revision or generation.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The independent evaluation is supplied through the user. No reproduction,
projection execution or additional independent review ran in this edit pass.

- [RemediationTarget][target] has only `CanonicalDomainName { object_id }`.
  No message, claim or raw-tool remediation target is inferred from that enum.
- [remediate_text_inner][remediate] loads the object at lines 409-410, checks
  domain kind, and updates `domains.name` at lines 418-419. The SQL does not
  update `source_revision`; the loaded object enters `PendingChange` unchanged.
- Lines 425-437 record target, operator, time and replacement, with change kind
  `operator_remediation`. Repeating a replacement can append only an audit event.
- [Live-domain test][live-test] checks two name replacements, audit fields and
  the caller receipt. [Retired-domain test][retired-test] checks retired-name
  replacement and repeated audit events. Both are unaudited adjacent checks.
- [RP2.1 U4][plan] requires source-byte fidelity through ingest/revise/delete
  and rebuild. [Canonical authority][authority] forbids changing canonical truth
  to fit the projection. These remain documented claims under test.
- [Embedding vocabulary][embedding-key] includes exact input hash in work
  identity. A fresh input hash can therefore discriminate changed mapped bytes.

Reachability: `test-only`. Canonical remediation exists, but no approved RP2.1
mapping or production projection path shows that `domains.name` contributes to
an input. The property becomes constructible only when that dependency is
specified. The domain-name witness does not establish that all fields of an
occurrence tuple, much less all fields of embedding identity `K`, remain equal.

## Failure scenario

Assume an approved bounded mapping `M` includes `domains.name` in a selected
projection or embedding input. An occurrence is materialized, then authorized
remediation replaces the name. A consumer that treats source revision alone as
input freshness can keep old payload bytes or call the old vector current.
Replaying older work can create the same error after otherwise correct catch-up.

The competing explanation is correct current-input hashing: the consumer
reconstructs input from current canonical fields and detects a changed hash.
Another explanation is that approved `M` never uses the domain name. Either
defeats an unconditional claim of a projection bug. Both must be checked before
calling the conditional failure reachable. No equality of all `K` fields is
assumed, and no revision/generation mechanism is mandated to defeat the bug.

## Timing windows and dependencies

Capture canonical field bytes and metadata before and after remediation commit
`r`. Evaluate catch-up and rebuild at a declared current state through `r` or
later, not by mislabelling a lagging historical snapshot as current authority.
For every affected occurrence, independently reconstruct `M(current)` within
approved row/byte bounds and compare exact payload bytes and current-input hash.
A stored payload hash copied back from the projection is not an independent
oracle. Vector currentness also requires the embedding owner's `D(o,m)` checks.
This property constrains currentness; it does not define at-rest residue cleanup
for old payloads, vectors, WAL or historical state, nor an erasure deadline.
Sensitivity admission and retention policy remain explicit owner decisions.

## What a test must construct

1. An approved field-to-input mapping with a demonstrated `domains.name`
   dependency, finite source selection and approved pre-materialization bounds.
2. A non-placeholder name, its source revision and independently captured input
   bytes before remediation; then the replacement bytes and unchanged revision.
3. Actual before/after occurrence tuples. Exercise a stable-tuple case only if
   approved mapping admits it; do not silently keep a changed span fixed.
4. Independent current-input hash computation and direct byte comparison,
   including an old payload/vector retained while the canonical field changes.
5. Catch-up, rebuild and pre-remediation work replay followed by currentness
   observations. Old bytes must not regain current status through replay.
6. Repeated remediation with no further byte change as a control, rather than
   requiring another embedding simply because an audit event exists.
7. Marker `search_projection_remediation_without_revision_change` from
   [fault-map](../fault-map.md), with canonical field and input-change witnesses.

## Investigation log

### Q: Which approved source mapping actually uses the remediated field?

- Sources examined: [target][target], [implementation][remediate], RP2.1 R1/U4
  and [embedding identity][embedding-key].
- Findings: Only domain-name remediation is implemented. The source classes
  are named, but their approved field-to-input mapping is not established.
- Missing evidence: Bounded `M`, affected occurrence enumeration and independent
  current-input construction. A proposed test fixture is not mapping approval.
- Conclusion: needs human input from canonical, projection and embedding owners.
  The conditional claim remains active; applicability is unresolved, not passed.

### Q: What at-rest residue policy follows from remediation?

- Sources examined: canonical remediation and the plan's authority/fidelity claims.
- Findings: The field update and currentness obligation do not specify deletion
  of every historical byte copy or authorize its retention.
- Missing evidence: Approved source sensitivity and residue policy, including
  WAL, retained vectors and history boundaries.
- Conclusion: needs human input. No universal erasure SLA, source exclusion or
  storage permission is added by this record.

[target]: ../../../../../crates/kernel/src/envelope.rs#L235-L242
[remediate]: ../../../../../crates/kernel/src/envelope.rs#L395-L438
[live-test]: ../../../../../crates/kernel/tests/kernel_retention.rs#L395-L466
[retired-test]: ../../../../../crates/kernel/tests/kernel_retention.rs#L743-L814
[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L151-L163
[authority]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L195-L210
[embedding-key]: ../../embedding/catalog.md#check-vocabulary
