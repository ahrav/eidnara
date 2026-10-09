# cf-delivery-scenarios-reach-qualified-invocations

## Discovery trigger

The method requires independent situation evidence so optional paths cannot
pass vacuously. R6 needs actual publication and delivery, including rare tier,
pressure, hint, and memory states. Existing-test, lifecycle, failure, replay,
and final wildcard passes produced this record. Inspected 2026-09-19 at
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8` in `ahrav/eidnara`.

## Evidence trail

- [rust-fold-under-pressure.test.ts:15-20,42-89](../../../../../packages/e2e-tests/tests/rust-fold-under-pressure.test.ts#L15-L89)
  gates the real scenario while an inactive-path test asserts that the switch
  is false. The real scenario checks shrink and nonempty history, not C1-C6.
- [rust-scenario-support.ts:12-18](../../../../../packages/e2e-tests/src/rust-scenario-support.ts#L12-L18)
  requires explicit `EIDNARA_E2E_FOLD=1` for broad fold qualification.
- [hermetic-host.test.ts:225-307](../../../../../packages/e2e-tests/src/rust-runner/hermetic-host.test.ts#L225-L307)
  checks readiness/control and completion counters. It does not establish a
  published nonempty history segment. Status of all these checks: unaudited.
- [transform.rs:21123-21228](../../../../../crates/daemon/src/transform.rs#L21123-L21228)
  supplies a seeded baseline/later-segment SOFT example with warm replay.
  It is a construction lead for m1, not end-to-end producer qualification.
- [transform.rs:8342-8364](../../../../../crates/daemon/src/transform.rs#L8342-L8364)
  makes hint rarity an independent prerequisite. A singleton candidate fixture
  cannot reach selection just by containing the query words.
- [canonical_memory.rs:195-211](../../../../../crates/daemon/src/canonical_memory.rs#L195-L211)
  filters and trims before rendering. Negative memory states must be observed
  before the provider assertion; absence alone cannot prove which state ran.
- [rust-harness.ts:457-496](../../../../../packages/e2e-tests/src/rust-harness.ts#L457-L496)
  provides bounded polling. Its default timeouts are helper behavior, not an
  approved latency promise for compression or producer completion.

Reachability is **test-only**: the record requires a bounded campaign to
construct a declared situation set. It does not claim default execution of a
gated E2E scenario or introduce production instrumentation.

## Failure scenario

A campaign has a passing gate test and many parser/renderer tests but never
publishes usable history through the direct host. All “constraint absent”
assertions run against empty captures. Another campaign reaches m0 but not m1,
uses only importance 50 for pressure, or checks a candidate's absence without
ever producing an eligible positive memory. The claimed portfolio is incomplete.

An invalid alternative would require `sometimes(lost_constraint)` beside a
preservation invariant. That condition can fire only when correctness fails.
This record instead requires the inputs and reached delivery situations under
which preservation or exclusion must be checked.

## Timing windows and dependencies

Construct a baseline before later publication for m1. Reach m0 after an actual
fold and capture a subsequent warm request. For hints, use a new eligible tail
and enough distractors before its decision freezes. For memory, capture the
appropriate pinned read after the intended admission/scope/availability state.
Keep invalid-recipe and missing-capture controls outside positive fidelity credit.

## What a test must construct

The exact constant markers and independent predicates are in
[fault-map.md](../fault-map.md#independent-coverage-checks-to-add). Apply
`sometimes` independently to all 14 markers and report a witnessed/missing
outcome with evidence for each. Required case/tier and negative/drop subcases
also retain separate outcomes; no aggregate boolean can hide an unobserved one.
The required observations include:

1. P1 in m1 and P1-P4 in m0, with accepted native source replaced.
2. Natural P5 and separate positive-budget high-importance pressure inputs.
3. Missing P2/P3, empty P4, and a persisted legacy/non-tiered input.
4. Hints-enabled selection beyond the fragment cap, total fragment drop after
   compression, and matched hints-disabled omission. The two fragment subcases
   are reported independently. Do not require the unreachable total-cap branch.
5. Independently admitted memory plus distinct candidate-only, explicitly
   rejected, wrong-project, withheld, and budget-excluded variants.
6. Applied warm repetition, raw fallback, and absent/unrelated capture controls.

Require C1-C6 P1-P4 coverage and at least one pressure/omission scenario each;
use focused extras, not pairwise reduction. Preserve U1 -> U2 -> U3 and consume
the evaluation-owned initial qualification receipt before per-invocation credit.
Record authored, parsed, and effective forms separately. A bad semantic output
can still establish a situation without receiving semantic acceptance.
All exercise remains **not yet**. Missing markers mean generator/setup gaps or
reachability regressions, not refuted formal liveness and not successful fidelity.

## Investigation log

### Q: Does the current fold test prove required default CI coverage?
- Sources examined: scenario, environment gate, mode manifest, fixture backend.
- Findings: the scenario is conditional and the inactive gate test can pass.
  The fixture's default backend output does not publish summary XML.
- Missing evidence: an ungated qualified fidelity lane and its actual run.
- Conclusion: resolved with answer: no execution or CI coverage credit here.

### Q: What attempt and timeout limits should qualification use?
- Sources examined: supplied plan U3 and bounded Rust harness polling.
- Findings: the plan requires bounded qualification and reporting a dependency
  if broader runtime work is needed; helper defaults do not choose campaign limits.
- Missing evidence: a declared run configuration approved before execution.
- Conclusion: needs human input. Do not extend limits after observing failures.
