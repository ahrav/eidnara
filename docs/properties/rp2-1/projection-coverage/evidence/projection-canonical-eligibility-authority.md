# projection-canonical-eligibility-authority

## Discovery trigger

The security lens finds a prerequisite rather than a landed shared module.
RP2.1 line 101 requires moving verdict policy into kernel, then sharing that
authority across daemon and retrieval adapters. A projection may retain data,
but it cannot turn its cached policy fields into canonical permission.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
No exploit, incident or parity run was supplied or executed.

- [RP2.1 ownership][plan] assigns canonical facts and shared eligibility to
  kernel and authorization/adapter lifecycle to daemon.
- [Index][index] repeats the shared-owner prerequisite and keeps checkout
  applicability separate from canonical eligibility.
- [Live judge][judge] is private in daemon. It orders absence, supersession,
  invalidation, revision, scope, sensitivity, visibility and artifact checks.
- [Cache key][key] includes lease epoch, tip, classification generation,
  candidate identity/revision/artifact, destination and project scope.
- [Snapshot evaluation][evaluate] redoes mixed cache hits and misses when
  canonical facts move. [Runtime guards][guards] constrain batch input.
- [Kernel modules][kernel] expose admission and applicability but no shared
  eligibility module. The tracked tree confirms the proposed module is absent.

Reachability: `test-only`. The daemon eligibility route is live, but the
shared kernel policy plus in-process retrieval adapter comparison has no
production path at HEAD. Existing one-adapter tests cannot reach that pair.

## Failure scenario

A projection copies a grant made at an older revision, then uses it after
canonical retirement, supersession or a sensitivity change. A second policy
implementation can also drift from daemon behavior for hidden or foreign-scope
objects even when both read fresh canonical data.

A competing explanation for unequal results is unequal inputs: one adapter
reads a later tip, another destination or another project binding. The parity
oracle must freeze the complete canonical context and preserve candidate order.
Test later revalidation separately so a legitimate time difference is not
misclassified as policy drift.

## Timing windows and dependencies

The policy move is an explicit implementation prerequisite, not a test-only
mocking interface. Dependency inspection must show both adapters use the same
kernel policy owner; behavioral equality alone cannot prove absence of a copy.
Authorization remains at the daemon boundary. Checkout applicability remains
a distinct decision and cannot override a canonical denial.
Cached and uncached adapters must bind their verdicts to the same canonical
snapshot identity. A projected checkpoint alone is not that identity.

## What a test must construct

1. A mixed ordered batch with a current eligible candidate and examples of
   stale revision, retirement, supersession, wrong scope and hidden visibility.
2. Local and remote destinations with sensitivity/artifact eligibility cases.
3. The same facts, snapshot, destination and scope supplied to both adapters.
4. A cached positive projection retained across a canonical retirement, then
   current validation that must not accept the projected grant.
5. Cache hits and misses while a source tip changes, reusing the existing
   daemon test seam where appropriate rather than introducing another cache.
6. A source/dependency check that both adapters reach the shared policy owner.
7. The paired-adapter and stale-grant markers in [fault-map](../fault-map.md).

Existing [stage1 checks][tests] assert ordered verdicts and retirement cache
invalidation for daemon only. Private cache tests at
`crates/daemon/src/kernel_routes/eligibility.rs:536-629` cover adjacent
snapshot behavior. All remain unaudited; no test-form decision is made here.

## Investigation log

### Q: What shared batch contract preserves snapshot and verdict semantics?

- Sources examined: [ownership][plan], [index prerequisite][index],
  [judge][judge], [cache key][key], [evaluation][evaluate] and [kernel][kernel].
- Findings: The daemon implements the verdict ladder and snapshot-aware cache;
  the plan requires one shared kernel policy, but the public shared adapter
  contract is absent. Wire names and verdict literals are existing contracts.
- Missing evidence: Batch input/output ownership, snapshot identity and the
  adapter split that preserves wire ordering without copying verdict policy.
- Conclusion: needs human input from kernel and daemon owners. The future
  specification must preserve existing wire semantics and name the shared
  authority; this discovery does not propose a new wire method.

[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L98-L104
[index]: ../../../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md#L35-L54
[judge]: ../../../../../crates/daemon/src/kernel_routes/eligibility.rs#L122-L174
[key]: ../../../../../crates/daemon/src/kernel_routes/eligibility.rs#L66-L78
[evaluate]: ../../../../../crates/daemon/src/kernel_routes/eligibility.rs#L296-L373
[guards]: ../../../../../crates/daemon/src/kernel_routes/eligibility.rs#L387-L413
[kernel]: ../../../../../crates/kernel/src/lib.rs#L7-L32
[tests]: ../../../../../crates/daemon/tests/stage1_eligibility.rs#L87-L224
