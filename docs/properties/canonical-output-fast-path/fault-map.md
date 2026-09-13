# Fault and situation map

This map uses the [catalog IDs](catalog.md#index). All checks are unexercised
here. “Available” means a verified source seam or inspected fixture exists,
not that injection or assertions ran. Existing checks remain unaudited.

## Fault classes and availability

| Class | Availability at pinned HEAD | Boundary |
| --- | --- | --- |
| Emitted key order and shape | Existing private generic serializer fixtures and real WireMessage facade. | No temporal fault. Observe emitted decoded keys, not original-presence or Value map assumptions. |
| Empty/singleton objects | Default HarnessMeta emits `{}`; generic and nested Value fixtures can supply singleton objects. | Keep the fewer-than-two-fields guard before key decoding. |
| Late/child-only disorder | Typed tagged blocks provide production child disorder, but the typed message root is already disordered. | Only the private exactly ordered root isolates a false-negative root-only aggregate; both witnesses remain required. |
| Serializer failure | Private encode accepts controlled Serialize sources. Failing-prefix fixtures/observations are missing. | Test-only error injection; no production WireMessage error or transport failure is claimed. |
| Hidden output-sized allocation | Existing allocator and facade entry are reusable; owner-thread event/size/lifetime recording is missing. | Absolute attribution, not per-block slope or pointer equality alone. |
| Cache miss/hit/invalidation | Existing cache fixtures and production branches exist. Independent constructor/Arc observations are missing. | Live-tail omitted entries increment hits too. Require a retained positive value, not the counter; exclude fresh differential work. |
| Prepared length/cap/overflow | Public preparation tests and inconsistent segment seam exist. Full diagnostic matrix is missing. | Fix variant, cap, measurement/write call partitions when comparing. |
| Writer failure | Existing FailAfter fixture accepts some bytes then errors. | Preserve existing path-specific errors; no new short-write contract. |
| Cancellation/denial | Private settlement closures already model both cancellation cuts and failed reserve. | Preserve ordering and zero-body behavior; not post-publication retraction. |
| Direct publication/transport faults | Existing T1–T4 owners and HP1 work remain separate. | No new transport fault campaign, coordination state machine, or liveness property is required here. |

See [existing checks](existing-checks.md) for pinned source links and exact
assertions, and [wildcard](_lenses/wildcard.md) for witness refinements.

## Per-property requirements

| Record | Required faults or enabling state | Cheapest discriminating observation | Handoff |
| --- | --- | --- | --- |
| [S1](evidence/served-field-change-flag-matches-stable-permutation.md) | All six permutations; escaped identity/inversion; empty/singleton; key classes; stable duplicate probes. | Complete descriptor permutation from independently decoded keys plus changed flag. | test-strategy |
| [S2](evidence/served-order-decision-visits-every-object.md) | Early and later disorder; ordered parent/disordered child; exact private ordered-root witness. | Literal late-disorder bytes plus source verification of the complete non-short-circuit loop. Optional local visits only if needed. | test-strategy |
| [S3](evidence/served-unchanged-span-copy-is-identity.md) | Successful real tables in source order; nested empty/singleton/array/scalar/string/number shapes. | Real unchanged copier equals its own A. Use compact punctuation reasoning, not a general validator. | test-strategy |
| [S4](evidence/served-serialization-is-single-pass-and-error-terminal.md) | Canonical/disordered success; error before write and with an open nested object. | Per-emission-site visits and private finalization observation before either success return. | test-strategy |
| [S5](evidence/served-canonical-return-retains-a-without-b.md) | Cold canonical miss; large scalar/small metadata; both unescaped and escaped order. | A lifetime identity plus absolute allocation/size attribution and logical reorder-output bytes. | test-strategy |
| [S6](evidence/served-output-cache-hit-skips-construction.md) | Eligible positive entries in synthetic and live-tail callers. | Scoped independent constructor/encoder deltas and Arc identity; exclude reference rebuilds. | test-strategy |
| [S7](evidence/prepared-output-diagnostics-preserved.md) | Same-variant/partition cap crossing, overflow, length mismatch, writer failure, cancellation cuts, denial. | Exact error/length/code/message and reserve/write event observations in existing seams. | test-strategy |
| [C1](evidence/served-canonicalization-campaign-reaches-risk-classes.md) | Every declared key/nesting/identity/success/error class. | AND of campaign-accumulated independent precondition markers. | test-strategy |
| [C2](evidence/served-cache-campaign-reaches-miss-and-hit.md) | Canonical cold miss, typed and edited disorder, two positive-hit callers, separate prepared replay. | AND of accumulated input/cache/completed-attempt state markers. | test-strategy |

## Coverage checks to add

The [C1 marker table](evidence/served-canonicalization-campaign-reaches-risk-classes.md#what-a-test-must-construct)
and [C2 marker table](evidence/served-cache-campaign-reaches-miss-and-hit.md#what-a-test-must-construct)
are the complete required situation sets. Each name is constant and globally
unique. Composite markers require every named member, including all six
permutations and both success/error classes. Accumulate across the campaign;
incompatible situations need not coexist in one call.
Report each missing marker separately, including any incomplete member of a
composite marker. Do not hide the missing situations behind one opaque result.

Markers inspect independent preconditions and must fire on correct code.
Never mark wrong ordering, unexpected B, construction on a hit, or a partial
published frame as desired coverage. No marker has fired in this pass. An
unfired marker requires investigation of missing construction or invalid
reachability assumptions, not a claim of formal liveness failure.

## Leverage ranking

1. **S1/S2 with C1:** extend existing private key fixtures and literal outputs.
   Mutating aggregation to short-circuit must fail the late-disorder witness.
   Use private test observations only where bytes cannot distinguish the claim.
2. **S3:** compare the existing copier with unchanged tables recorded by real
   encode. This isolates the ownership-transfer equivalence without another
   encoder, alternate production branch, or general span-validation subsystem.
3. **S4:** add two deliberate error cuts and separate site counts to the
   existing generic Serialize fixture. Observe finalization before either return.
4. **S5 with C2:** extend existing allocator support. This needs more care than
   byte comparisons because a fixed B escapes slopes and harness allocations
   contaminate process-global counts. Use the documented isolated libtest and
   nextest commands, owner-thread gating, and nonallocating recording.
5. **S6/C2:** extend cache fixtures with scoped call and Arc checks; do not count
   the intentional fresh differential as hit work.
6. **S7:** preserve existing tests first, then add missing diagnostic assertions
   at the same source variants and call partitions. Do not expand to ring tests.

Always-copy and short-circuit negative controls are proposed discriminators,
not executed results. Keep baseline binaries out of the candidate production
tree. No second production canonicalizer or benchmark-only legacy branch is
allowed. Existing tests and runtime guards need their separate adequacy audits.

## Measurement boundary

Plan U0/U4, not W1/W2, owns before/after measurements. Preserve the same harness
revision, 1/65-block allocation points, 100/1,000-message transform points,
retained/typed/edited shells, escaped keys, many tiny objects versus large
scalars, cold construction, and warm positive hits. Those sizes are controlled
fixtures, not verified production distributions. Record actual populations.

Separate allocation events, cumulative requested bytes, peak live bytes,
A length/capacity, logical reorder-output bytes, CPU time, and latency. A
reallocation event does not report physical bytes copied. Capture the full
constructor through receipt construction, hashing, and Arc conversion because
A slack remains live until then. Investigate an increased full-constructor
peak before landing; never infer lower peak from one fewer allocation.

No isolated canonicalizer or full-constructor benchmark cell exists at HEAD.
The integration facade covers only canonicalization. U0 must search/reuse the
allocator fixtures, then establish nonallocating thread-owned test-only
recording in an isolated filtered in-crate process that calls the existing
private constructor. No unit-binary allocator is installed at HEAD. Verify
this observer before U1, without a public constructor wrapper/API, production
hook, disabled cfg(test) differential, harness framework, or dependency. If
compatible test-only observation is unavailable, stop U0 for seam approval
rather than weaken the peak gate. The route is feasible, not executed proof.

B1 preserves final Arc payload length and bytes. A's transient slack survives
fingerprints and hashing and ends at Arc conversion. Unchanged retained
accounting does not bound global RSS; measured peak remains unknown, so no
universal residency win is claimed. U4 already requires investigation if
measurement changes the design; no extra transport work or record is needed.

Extend benchmark files without adding production APIs. Keep ten independent
AB/BA process pairs and the plan's
artifact/toolchain/feature/CPU/allocator/sample provenance. Timing results
limit the claims; deterministic no-B evidence alone proves no latency gain.

The direct_host seam exists, but a reproducible real-transform host benchmark
driver does not. Write that driver only if claiming user-visible latency,
including `serve_native` and send-to-correlated-terminal timing. In-process
hot_path and ipc_budget echo are not substitutes. No extra timing field,
direct-output caller, source lease, cache charge, or transport redesign is
authorized. Preserve every plan verification command and use CI as authority
for additional path-selected checks when implementation begins.
