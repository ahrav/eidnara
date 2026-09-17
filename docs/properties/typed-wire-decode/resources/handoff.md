# Per-property handoff

System: typed-wire decode resources.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](source-register.md) records evidence provenance.
These are downstream instructions, not work executed by this discovery.

Every active record goes to `/testing:test-strategy` for the cheapest valid
test form and observation boundary. Invalidated R6 is excluded from runtime
implementation scope. Existing tests go to `/testing:invariant-test-review`;
live admission guards go to
`/low-level-systems:defensive-assertions-and-invariant-guards`. No new public
test seam is assumed necessary when an existing one can observe the claim.

| Slug | Strongest existing seam and required next evidence |
| --- | --- |
| [decode-footprint-covers-both-lanes-combined-peak](evidence/decode-footprint-covers-both-lanes-combined-peak.md) | Reuse the isolated peak binary and public ResidentMeter/ResidentReserve. Check combined direct/tree/failure/fallback `P <= F`. KTD4 coefficient selection stays separate: start strings at one, raise to smallest passing value, keep node copies two. Explicitly disposition the hard-coded-three test at `crates/daemon/src/lib.rs:20281-20315`; it stays unaudited. |
| [retained-accounting-follows-typed-ownership](evidence/retained-accounting-follows-typed-ownership.md) | Reuse in-crate retained-size, FlatProjection, and native-cache sharing controls. Build an independent ownership ledger for all retained payload Values, canonical text, capacities, and Arc owners. Audit representation-specific assertions before replacing them. |
| [frozen-admission-outcomes-and-boundaries-stay-stable](evidence/frozen-admission-outcomes-and-boundaries-stay-stable.md) | Freeze original A1-A3 bytes, numeric capacities, pressure, and outcomes. Keep length caps, node floor, error mapping, one-terminal emission, and no dispatch effects unchanged. Add new string-ceiling witnesses under KTD4 without replacing old cases; stop if an original case changes. Count terminals before managed-client filtering. |
| [decode-and-projection-stay-within-resident-pool](evidence/decode-and-projection-stay-within-resident-pool.md) | Observe complete demand under existing logical pool and holder rules, including canonical workspace and cleanup. Expose the verified above-cap probe/A1 ordering discrepancy; quantify its allocation only with a measured witness. Preserve ownership and accounting policy; require no new exact RSS or allocator model. |
| [message-decode-allocation-gate-has-isolated-scope](evidence/message-decode-allocation-gate-has-isolated-scope.md) | Reuse allocation counter patterns after auditing scope and hook completeness. Choose production-subtree attribution or isolated IngressMessages with integration equivalence. Freeze 40-message JSON and enforce `events <= 640`, `peak < 3 * J`. No mirror decoder or peak subtraction. |
| [decode-projection-payoff-has-comparable-evidence](evidence/decode-projection-payoff-has-comparable-evidence.md) | Invalidated for category mismatch. [EG1](evidence-gates.md#eg1-decode-projection-payoff) is retired; its passing verdict is withdrawn after removal of the measurement evidence. No campaign is pending. Future payoff claims need new retained evidence and an owner decision. W1 stays invalidated. |
| [resource-witnesses-reach-independent-preconditions](evidence/resource-witnesses-reach-independent-preconditions.md) | Reuse TestPool::hold, ShortfallMarker, tree-stage entry, and Arc identity controls. Implement twelve independent sometimes checks under their original names; aggregate only for reporting. EG1's measurement-pair receipt is retired and excluded from runtime coverage. Scheduling machinery is considered only if test-strategy needs it. |

## Blocking decisions carried forward

1. P:L176 witness retuning conflicts with preserving original A1-A3 cases.
   P:L88's wider string ceiling is accepted. If an original case changes,
   stop and obtain an owner disposition; new ceiling witnesses are additive.
2. P:L213 asks for W1 updates while its owning catalog invalidates it. The
   plan-local evidence gate neither edits nor reactivates W1.
3. Messages-only allocation attribution is unspecified by the whole-request
   test proposal. The threshold cannot be applied until scope is valid.
4. A1's charge-before-probe claim conflicts with verified above-cap ordering.
   Owner disposition, a quantitative witness, and attribution of probe and
   projection demand to existing logical pool coverage remain open.
5. EG1 is retired. A future payoff campaign needs a new owner decision,
   artifact owner, and predeclared noise rule.

## Independent evaluation and remaining work

The user supplies the four-lens independent report from analyst
`ses_f6756093fffeVjNp36S3E8pKrM`. Local source validation and all dispositions
are in [portfolio-evaluation.md](portfolio-evaluation.md). The review is
complete; runtime exercise, test-adequacy audits, and owner decisions remain
open. EG1 execution is retired, not pending. No liveness property is invented
for this synchronous slice or for the measurement process.
