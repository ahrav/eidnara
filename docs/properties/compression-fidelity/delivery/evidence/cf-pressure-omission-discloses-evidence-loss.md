# cf-pressure-omission-discloses-evidence-loss

## Discovery trigger

R3 requires visible, discoverable, or unavailable evidence to be assessed on
the actual invocation. R9 does not permit preserving everything by expanding
budgets. Resource, failure, and product lenses identify the conflict: bounded
serving can correctly omit a segment while failing its useful-retention claim.
Inspected 2026-09-19 at `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.

## Evidence trail

- [decay_render.rs:413-416](../../../../../crates/daemon/src/decay_render.rs#L413-L416)
  returns empty for P5. Stored existence therefore does not imply prompt text.
- [decay_render.rs:491-531](../../../../../crates/daemon/src/decay_render.rs#L491-L531)
  computes age/importance tiers and passes literal `0.0` for anchor overlap.
  Core tests with nonzero overlap cannot establish production protection.
- [context-core/decay.rs:132-149](../../../../../crates/context-core/src/decay.rs#L132-L149)
  can keep a naturally archived row at P4 when anchor overlap protects it.
  This capability is not used by the daemon call above.
- [decay_render.rs:575-596](../../../../../crates/daemon/src/decay_render.rs#L575-L596)
  demotes the oldest nonarchived row until its positive budget fits or the guard
  exhausts. The loop does not exempt high importance or the newest-tier floor.
- [decay_render.rs:835-852](../../../../../crates/daemon/src/decay_render.rs#L835-L852)
  tests an 80-character budget with importance 50 rows; status unaudited.
- [m0_compose.rs:138-167](../../../../../crates/daemon/src/m0_compose.rs#L138-L167)
  adds at most three pressure retries for wrapped history above 105% of budget.
  This trigger is neither a hard-budget allowance nor a guaranteed cap. The
  [resource record](../catalog.md#cf-serving-resource-boundaries) owns that distinction.
- [transform.rs:8288-8317](../../../../../crates/daemon/src/transform.rs#L8288-L8317)
  searches stored history across bodies and tiers without consulting the
  selected render tier. An omitted segment may still contribute a hint.

Reachability is **default-production**: the default compaction setting is true
at [config.rs:144](../../../../../crates/daemon/src/config.rs#L144), and normal
m0 composition calls the renderer. A deliberately tight positive budget is a
test input to an existing path, not a proposed production policy change.

## Failure scenario

A high-importance prohibition appears only in an older segment. Hard pressure
removes that row despite its importance. The report sees a smaller request and
marks preservation successful because the source is still stored or because
the consumer refuses to answer. Neither observation establishes retention.

Alternatively, a live-tail duplicate or an independently admitted memory row
retains the constraint. The invocation can be meaningfully supported, but the
report must name the actual surface. A truncated hint does not inherit the
complete stored summary's authority or recovery availability.

## Timing windows and dependencies

Natural archival and guard-induced omission need separate observations.
Record the unguarded selection and positive requested budget before final
rendering. Check both hints-enabled and hints-disabled invocations after the
source range leaves the tail. Preserve warm/cold observations and actual size.
The registered-tool owner supplies evidence for discoverability; fixture-only
reads cannot qualify it. Consume
[cf-unavailable-evidence-no-credit](../../recovery/catalog.md#cf-unavailable-evidence-no-credit)
as the canonical disposition/credit predicate. This record supplies the omitted
range, pressure mechanism, and lost obligation, then preserves any required-
retention failure. It does not implement another validator. Native exact
recovery is outside this record.

## What a test must construct

1. Publish a qualified source with a one-off material constraint and high
   importance. A single-row history isolates guard pressure: its curve age is
   zero at [decay.rs:55-76](../../../../../crates/context-core/src/decay.rs#L55-L76).
2. Supply a positive budget below the independently charged nonarchived forms.
   The curve selection at that same budget must still be renderable. Observe
   guard-induced removal without changing anchor overlap or importance policy.
3. Construct natural P5 separately, then capture the complete request after
   accepted-range replacement with no decisive raw-tail copy.
4. Pass the correlated obligation and complete-invocation observations to the
   canonical predicate and consume its disposition and credit result.
5. If useful context is required and unavailable, retain the preservation
   failure despite safe abstention or smaller output. Where the scenario instead
   explicitly permits unavailable evidence and abstention, acceptance remains
   possible without useful-preservation or recovery credit. Keep the outcomes
   separate. Nonmaterial detail must be annotated before evaluation.

The pressure marker asserts a renderable high-importance input and insufficient
budget, not semantic loss. All exercise remains **not yet**.

## Investigation log

### Q: Does importance or anchor protection prevent hard omission?
- Sources examined: daemon tier selection, core archival protection, hard guard.
- Findings: the daemon supplies zero overlap; the hard guard scans by age order
  and does not consult importance. A core protection test is a competing but
  inapplicable explanation for production retention.
- Missing evidence: a qualified high-importance provider capture.
- Conclusion: resolved with answer: do not assume either protection in delivery.

### Q: When may unavailable evidence be accepted?
- Sources examined: supplied plan R3 and its recovery/omission rule.
- Findings: safe abstention and preservation are distinct. Losing a sole useful
  constraint still fails required preservation; an explicitly permitted
  unavailable/abstention outcome is not categorically banned from acceptance.
- Missing evidence: approved per-scenario retention and abstention obligations.
- Conclusion: needs human input for the case annotations, not a budget increase.
