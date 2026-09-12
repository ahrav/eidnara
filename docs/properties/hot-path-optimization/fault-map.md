# Fault and enabling-state map

The system is `/local/home/ahrav/scratch/eidnara`, at
`913234433ae36a80a6e22c6aac14c7f9aab74386`, checked on 2026-09-10.
The supplied audit and repository evidence define the
[pending-confirmation scope](catalog.md#scope-and-provenance). No external
plan or incident report is supplied. No fault campaign runs here.
This is a working-tree supplement, not part of the source baseline commit.

## Fault availability

| Class | Construction and availability | Limit |
| --- | --- | --- |
| Selection pressure | Valid fixture rows can exceed the [row/byte caps][caps]. | Row and byte pressure require separate K3/K4 witnesses that cannot mask each other. |
| Authority changes | Admission, scope, and sensitivity facts feed the [served-row fold][fold]. | Corrupt excluded-row errors remain an unresolved pushdown gate. |
| Cancel and close | [Dispatch][dispatch] exposes cancellation and a [close gate][close]. | Current callbacks are reachable; future worker-specific test readiness is BLOCKED. |
| Retained resources | The [current ledger][ledger] distinguishes pending/task counts, ingress/parse bytes, and staging/decode guards. | Future queued/running/result mapping and numerical capacity remain owner gates. |
| Maintenance and unwind | [Guarded scopes][scopes] coexist with maintenance and panic restoration. | A setup-elision plan must identify all authority invalidations. |
| External commit | The [snapshot test][snapshot] commits on a second SQLite connection. | Batching cannot force independent calls into one old snapshot. |
| History pressure | [The renderer][history] accepts ordered compartments and an estimator. | A character estimator does not establish production BPE decisions. |
| Wrapped retry pressure | [The outer retry][outer] retains a wrapper even for an empty body. | H4 separately witnesses the initial 105% trigger and requires a boundary matrix. |
| Preparation refusal | [The preparer][prepare] rejects detected identities and size violations. | Input refusal and output expansion must be distinguished. |

## Required state per property

| Record | Required faults or enabling state | Independent observation |
| --- | --- | --- |
| [K1][k1] | Snapshot-bound admissions, uncertain scope, tie order, payload pressure, and withheld serving. | The baseline result includes bytes, IDs, metadata, and outcome. |
| [K2][k2] | Valid unrelated rows precede eligible memory without changing its authority. | Paired relevant facts and selected output are compared. |
| [K3][k3] | More than 8192 admitted visible rows put eligible memory beyond rank 8192 in unfiltered serving order; all stored payloads total less than 8 MiB. | Seed count/rank witnesses row pressure with no masking byte cutoff. |
| [K4][k4] | At most 8192 candidates include excluded preceding decision BLOBs totaling over 8 MiB; the relevant prefix through the target fits. | Independent stored BLOB lengths witness byte pressure with no masking row cutoff. |
| [E1][e1] | Close overlaps a held current callback; moved work must extend the gate. | Callback completion precedes cleanup/reuse; worker-specific proof is blocked pending design. |
| [E2][e2] | Cancellation, refusal, result retention, and retry transfer occur with charged resources. | A class-specific ownership ledger follows the actual resource. |
| [E3][e3] | Work starts, close begins, and work has not completed. | Separate start/close/completion events establish overlap. |
| [S1][s1] | Maintenance creates a shadow, SQL is cached across authority changes, and callbacks fail or unwind. | Current-scope effects/refusals and subsequent connection state agree. |
| [S2][s2] | A second connection commits within a callback and between callbacks; one independent write fails. | Read values and original atomic effect sets agree with the baseline. |
| [H1][h1] | Unicode, escaping, joins, repeated tier bytes, and removal affect selection. | Final bytes and actual whole-body costs match a frozen reference. |
| [H2][h2] | Positive/nonpositive direct budgets, ordinary SOFT, pressure refold, and pure Defer/SoftPlus occur. | Ordinary SOFT preserves m0, not all units; pure replay preserves its retained prefix. |
| [H3][h3] | The reference remains over budget after its first legal demotion. | Actual reference costs and remaining demotability witness setup. |
| [H4][h4] | The initial reference wrapped slice exceeds 105% of a positive budget. | Record actual slice cost; separately cover equality, bypass, and three-attempt exhaustion. |
| [R1][r1] | All six policy/layer pairs receive clean and detected input. | Outputs/refusals and normalized audit metadata agree. |
| [R2][r2] | An earlier field succeeds before identity or input/output-size refusal. | No scan append occurs for the refused field; failed writes roll back. |
| [R3][r3] | Equal prepared detection/policy/owner graphs cross success, replay, refusal, and rollback. | Audit projections agree regardless of retained redacted text; privacy remains a separate existing contract. |

## Coverage checks to add

K3, K4, E3, H3, and H4 use their slugs as constant, globally unique marker
names. Each asserts input or scheduling preconditions, never the defect.
K3 and K4 are both required, not alternatives, and do not require correct output
truncation. K4 measures stored decision_payload BLOB bytes, including their JSON
encoding, separately from summary characters and response serialization bytes.
E3 does not require early cleanup. H3 does not require separate candidate loop
iterations or cache misses. H4 witnesses the initial reference retry predicate,
not a failed return; budget 0.5 with empty history supplies the separate
source-derived three-attempt exhaustion case in [its matrix][h4-evidence].

Other records need explicit case accounting: admission and scope variants for
K1/K2, resource classes for E2, maintenance/authority transitions for S1/S2,
budget surfaces for H2, and all six policy/layer combinations for R1/R2.
Missing case accounting remains missing evidence even if assertions pass.

## Cheapest valid oracle first

1. R1/R2 and H1/H2 have direct value/metadata comparisons. Start with existing
   fixtures and independently frozen expected results, not a new simulator.
2. K1/K2 and S1/S2 need real SQLite boundaries. Paired stores and a controlled
   second connection are cheaper than a distributed harness.
3. K3/K4 and H3/H4 strengthen comparisons with independent pressure witnesses.
4. E1-E3 need ownership and physical-completion observations. Resolve execution
   topology before selecting a timing harness; sleeping and polling health
   counters are not substitutes for those observations.

This ranking routes work to `/testing:test-strategy`; it does not choose final
test forms or set a benchmark acceptance policy. No new global duration or
per-job bound is invented. Existing close budgets preserve the fatal branch.
Lost responses require per-operation reconciliation under existing CAS/receipt
semantics, not an exactly-one-effect assertion for each transform.
The [fixed baseline identity][reference] settles semantic acceptance; choosing
an independent executable or adapter only packages it. Excluded-row error
suppression remains unapproved before M1. Performance workloads, measurement
policy, M5 allocation acceptance, and final external scope remain owner gates.

[caps]: ../../../crates/daemon/src/kernel_routes/read.rs#L23-L26
[fold]: ../../../crates/kernel/src/admission.rs#L3234-L3297
[dispatch]: ../../../crates/host-runtime/src/dispatch.rs#L823-L998
[close]: ../../../crates/host-runtime/src/dispatch.rs#L1237-L1268
[reservations]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L513-L550
[scopes]: ../../../crates/storage/src/lib.rs#L648-L729
[snapshot]: ../../../crates/storage/src/lib.rs#L3972-L4007
[history]: ../../../crates/daemon/src/decay_render.rs#L296-L338
[prepare]: ../../../crates/memory-store/src/lib.rs#L2198-L2237
[outer]: ../../../crates/daemon/src/m0_compose.rs#L178-L215
[ledger]: evidence/request-work-accounting-covers-retained-resources.md#evidence-trail
[reference]: catalog.md#fixed-reference-identity
[h4-evidence]: evidence/history-outer-retry-pressure-is-exercised.md#what-a-test-must-construct
[k1]: catalog.md#canonical-memory-result-preserves-current-selection-pipeline
[k2]: catalog.md#canonical-memory-irrelevant-candidates-do-not-consume-caps
[k3]: catalog.md#canonical-memory-selection-pressure-is-exercised
[k4]: catalog.md#canonical-memory-byte-pressure-is-exercised
[e1]: catalog.md#route-cleanup-waits-for-request-owned-physical-work
[e2]: catalog.md#request-work-accounting-covers-retained-resources
[e3]: catalog.md#request-close-overlaps-live-work
[s1]: catalog.md#guarded-callbacks-enforce-current-authority
[s2]: catalog.md#callback-batching-preserves-observation-boundaries
[h1]: catalog.md#history-budget-selection-preserves-reference-bytes
[h2]: catalog.md#history-budget-boundaries-remain-distinct
[h3]: catalog.md#history-budget-pressure-paths-are-exercised
[h4]: catalog.md#history-outer-retry-pressure-is-exercised
[r1]: catalog.md#prepared-field-output-and-audit-policy-agree
[r2]: catalog.md#preparation-refusal-does-not-append-audit-state
[r3]: catalog.md#redaction-audit-does-not-depend-on-retained-payload
