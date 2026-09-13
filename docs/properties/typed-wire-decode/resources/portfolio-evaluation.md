# Portfolio evaluation and dispositions

System: typed-wire decode resource catalog.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and sources: [source-register.md](source-register.md).

## Review provenance and status

The user supplies nine findings from independent analyst
`ses_f6756093fffeVjNp36S3E8pKrM` and reports all four portfolio lenses complete.
This file records that independent report, local validation against P and HEAD,
and the resulting dispositions. Local validation is not presented as a second
independent review. No implementation test or benchmark runs.

The pending placeholder is superseded. Before this revision, catalog SHA-256
is `a6a68de097733082e9bc85ef82e1c608b0d8b7bf4387f44edcdab1a1e2bd613e` and
portfolio placeholder SHA-256 is
`02d81ffef99992636d13be7dc4aa169c9d7cf4e38640c8b4f1884ec9557b6022`.
Original lens notes stay under `_lenses/` as discovery snapshots. Where they
conflict with these dispositions, the revised catalog and this file control.

## Four completed lenses

| Lens | Independent finding focus | Local validation and disposition |
| --- | --- | --- |
| Harness fit | Payoff is an evidence gate; allocation thresholds are real resource invariants; no runtime liveness for measurement. | R6 is invalidated for category mismatch and moved to EG1. R5 stays active with an exact budget predicate and separate instrumentation rules. |
| Coverage balance | Blanket frozen boundaries contradict KTD4; original cases and fixed gates still need preservation. | R3 partitions length caps, node floor, error/terminal/effect rules, original frozen A1-A3 cases, and additive string-ceiling witnesses. No liveness claim is added. |
| Implementability | Coefficient selection is not the invariant; hard-coded-three assertion needs linkage; each marker needs an independent sometimes check. | R1 checks `P <= F`; KTD4 selection stays outside Check. The existing test is linked and its reported omission qualified. Twelve resource checks retain names and separate results. |
| Wildcard | Probe ordering contradicts A1; do not turn accounting into a new ownership or exact RSS model. | Ordering conflict is confirmed from HEAD. Allocation size remains unmeasured. R2/R4 preserve existing logical pools, alias accounting, and conservative holder policies. |

## Numbered finding dispositions

IDs below match the nine findings supplied by the user. Classification is
for disposition: seven refinements, one gap, and one bias. Open measurements
and owner questions do not mean the four-lens review is still pending.

| ID | Class | Validation | Disposition |
| --- | --- | --- | --- |
| 1 | refinement | P:L88 accepts wider string admission, while P:L18 and P:L70 preserve original witnesses. The old all-boundary reading exceeded that decision. | Applied to R3, its evidence, fault map, and handoff. Freeze original bytes/capacity/outcome and stop on change; keep length caps/node floor/error mapping/one terminal/no effects fixed. Add new ceiling cases without replacement. P:L176 retuning remains an owner question. |
| 2 | refinement | P:L213 defines a measurement artifact; the R5 event/byte thresholds instead bound a concrete decode operation. W1 is invalidated at the owning HEAD record. | R6 remains as invalidated history, with exact payoff obligation in evidence-gates.md as EG1. R5 stays active with `E_msg <= 640` and strict `P_msg < 3 * J`; scope validity is an instrumentation requirement. No W1 reactivation. |
| 3 | refinement | P:L88 fixes node copies at two and supplies a coefficient-selection procedure. A pass/fail heap observation is only `P <= F`. | Removed coefficient search from R1 Check. Accepted design section and evidence retain start-at-one, smallest-passing-value selection and failed smaller-value observations. |
| 4 | refinement | HEAD `crates/daemon/src/lib.rs:20281-20315` exists; `:20294-20298` hard-codes three copies. The pre-disposition existing-checks table already lists it, so a table omission is not confirmed. R1/evidence omitted the direct link. | Applied the missing R1/evidence links and exact assertion anchor. Existing-checks logs this qualification rather than inventing a missing row. Test remains unaudited; candidate disposition remains prospective. |
| 5 | gap | `crates/daemon/src/lib.rs:12153-12166,16144-16161` proves the above-cap probe runs before meter creation, contradicting A1 at `docs/properties/hot-path-optimization/latency-audit/catalog.md:159-165`. | Documentation gap closed by exposing the verified discrepancy in R4, evidence, inventory, and fault map. Owner disposition and a quantitative allocation witness remain open. No allocation amount, OOM, or budget exceedance is diagnosed. |
| 6 | refinement | METHOD distinguishes individual situation checks from summary aggregation. One combined result cannot replace each marker's predicate and receipt. | Twelve independent resource sometimes checks retain their original names. Their conjunction is only a rollup. The unchanged measurement-pair name moves in category to EG1's completeness receipt, outside R7. |
| 7 | refinement | The resource slice supplies finite synchronous checks, not a new bounded recovery or request-latency guarantee. | Retained no-liveness disposition. Neither the benchmark nor its completion receipt is a runtime liveness property. |
| 8 | bias | `crates/host-runtime/src/config.rs:65-72` defines logical payload accounting, not exact RSS; existing holders deliberately use conservative charges. The earlier physical-envelope wording could imply a redesign. | Applied scope correction to R1/R2/R4 and evidence: preserve declared logical pools and ownership policy; retain requested-layout lower-bound caveat. No new global deduplication, ownership transfer, allocator policy, or exact RSS requirement. |
| 9 | refinement | The supplied independent report names four completed lenses and requires findings/dispositions with open questions. | Replaced pending placeholder with this report, source validation, audit identities, classification, and open-work ledger. Runtime exercise and prospective evidence remain pending separately. |

## Source validation anchors

- P retains SHA-256
  `badf0d718366bd627d453498576935ba2fd3292cfe5701b7020fa09a07c52f32`.
  P:L18, P:L51, P:L67, P:L70, P:L88, P:L164, P:L176, and P:L213 are reread.
- `crates/daemon/src/metered_decode.rs:33-64,419-435` confirms node copies two,
  string copies three at HEAD, and the string-byte-free node floor.
- `crates/daemon/src/lib.rs:20294-20298` confirms the hard-coded-three assertion.
  Pre-disposition inventory SHA-256 is
  `f06f27cffb73b6ef5b532b03fb24d761e0c4cbb528e6ce1eb3d42eaee7c7db77`.
- `crates/daemon/src/lib.rs:15837-15860` reads probe keys through serde strings.
  This plus call order proves the timing discrepancy, not allocation magnitude.
- `docs/properties/hot-path-optimization/latency-audit/catalog.md:142-148`
  explicitly limits one-terminal evidence because the managed client drops
  later terminals. A producer-side or unfiltered observer remains needed.
- `docs/properties/hot-path-optimization/latency-audit/catalog.md:1772-1773`
  keeps W1 invalidated; no other directory is edited to change that status.

## Open owner decisions

| Question | State and constraint |
| --- | --- |
| How should P:L176's retuning instruction be reconciled if KTD4 changes an original A1-A3 case? | Needs human input. Stop on that changed original case; never replace it. The wider string ceiling itself is already accepted. |
| How should A1's unqualified charge-before-probe statement be dispositioned for above-cap input? | Needs human input. The ordering discrepancy is verified. Preserve it until an owner resolves the contract; allocation magnitude remains unmeasured. |
| Which existing reservation covers probe and canonical/projection workspace under the logical budget? | Attribution remains open. This does not authorize a new ownership or RSS model. |
| How will the 40-message allocation observation isolate messages from native fields, setup, projection, and other threads? | Needs human input on instrumentation. Budget remains `<=640` and strict `<3J`; invalid attribution cannot pass. |
| Who owns the exact payoff artifact and predeclared noise/uncertainty rule across both sizes? | Needs human input. EG1 retains statistics -> experiment design -> bench compare routing and the whole-plan within-noise stop. |
| How should P:L213's requested W1 update coexist with its invalidated status? | Needs human input. EG1 stays plan-local; W1 remains invalidated. |

## Prospective evidence and gaps

No candidate measurements establish the smallest passing string coefficient,
combined direct/tree/failure/fallback peak, full logical pool bound, new text
ceiling, allocation threshold, or timing payoff. Original frozen byte/capacity/
outcome receipts and unfiltered terminal observations also need downstream work.
Those are evidence tasks, not permission to redesign accepted decisions.

All existing tests and production guards remain unaudited. No `Exercised`
field is promoted by this review. R6 is excluded from runtime implementation
and coverage counts, while EG1 preserves its entire accepted obligation.

## Final portfolio

- Active: six records, comprising five safety `always` records and one
  reachability record specifying twelve independent `sometimes` checks.
- Invalidated: one record, `decode-projection-payoff-has-comparable-evidence`.
- Evidence gates: one required gate, EG1; execution pending.
- Preserved names: twelve resource markers plus one evidence receipt.
- Liveness records: zero. Fresh portfolio review: complete as supplied;
  source validation and documentation disposition: complete.
