# cf-delivery-credit-requires-published-folded-capture

## Discovery trigger

R6 evaluates the served invocation, not the summarizer's intention. Architecture,
concurrency, dependencies, failure, protocol, and wildcard lenses identify
independent false-success routes. Inspected 2026-09-19 at
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8` in `ahrav/eidnara`.

## Evidence trail

- [direct_host_fixture.rs:383-498](../../../../../crates/daemon/examples/direct_host_fixture.rs#L383-L498)
  ignores the backend request and emits literal `fixture-success`, then increments
  `completed`. This is not summary XML bound to a case or its source aliases.
- [history_summarizer_validate.rs:588-592](../../../../../crates/daemon/src/history_summarizer_validate.rs#L588-L592)
  rejects output without usable history segments. Thus backend completion alone
  cannot witness publication. A different successful producer path remains
  possible; this observation is specific to the fixture implementation.
- [history_summarizer.rs:4154-4205](../../../../../crates/daemon/src/history_summarizer.rs#L4154-L4205)
  tests valid scripted publication in-process; status unaudited. Its presence
  does not prove the direct-host/OpenCode seam is qualified.
- [transform-session-client.ts:1512-1631](../../../../../packages/opencode-plugin/src/hooks/context/transform-session-client.ts#L1512-L1631)
  validates recipe, source, boundary, and invocation before replacing the host
  array. Failure reports raw serving. The input can still answer the follow-up.
- [rust-mode-transform.test.ts:1733-1829](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1733-L1829)
  checks raw input retention on missing/invalid recipes and bad boundary;
  status unaudited.
- [rust-harness.ts:577-601](../../../../../packages/e2e-tests/src/rust-harness.ts#L577-L601)
  filters main requests by the Eidnara system heading. Missing capture becomes
  `[]`, zero bytes, or serialized `[]` in convenience helpers.
- [mock-provider/server.ts:72-86,214-216,267-308](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L72-L86)
  defines full request captures, including system/messages/tools; its
  [reset](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L214-L216)
  deletes them, and its [handler](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L267-L308)
  records the received body. Retain observations before scripted helper reuse.
- [invocation-budget.ts:61-71](../../../../../packages/opencode-plugin/src/hooks/context/invocation-budget.ts#L61-L71)
  admits fitting, shrinking, and unknown-limit arrays. Preserve these existing
  rules; record their reason rather than inventing an exact provider token cap.

Reachability is **test-only**: this record governs offline attribution of
delivery evidence. The underlying production application/fallback paths exist,
but no production semantic credit counter or evaluator is proposed.
Initial script qualification belongs to
[cf-fixture-script-qualification](../../evaluation/catalog.md#cf-fixture-script-qualification).
This record consumes its successful identity-bound receipt, not its validator.

## Failure scenario

The fixture reports a completed backend call without publishing a summary.
A follow-up request still includes the raw conversation, so the answer is
correct. Alternatively, recipe rejection retains raw input after publication,
or a missing capture produces zero bytes and passes a shrink assertion.
Each can be reported as compression success unless the stages are joined.

Discarded coverage is another false attribution: an accepted earlier segment
does not qualify obligations from a provisional final segment still in the
tail. Compare accepted coverage with the exact source obligations under review.

## Timing windows and dependencies

Correlate case, session, source range, follow-up, transform application, and
provider request. Global last-request selection and aggregate counters are not
a join key. Publication may precede a rejected application; a provider capture
may precede a diagnostic log. Wait within a declared bound for both observations.
Warm repeated serving reuses bytes, not a new producer execution.
Even after initial qualification, each invocation still needs its own accepted
range replacement, applied recipe, nonempty correlated capture, and raw-tail
isolation. A receipt cannot substitute for those later observations.

## What a test must construct

1. Consume the evaluation owner's successful fixture-qualification receipt for
   the tested fixture/corpus. Missing or mismatched receipts leave delivery
   unqualified. Do not rebuild alias, queue, or script-exhaustion validation here.
2. Require accepted-range replacement and successful recipe application, then a
   nonempty correlated provider body. Read system, messages, and tools together.
3. Verify covered decisive source is absent from every live-tail block for
   compression-only credit. Record any remaining source as alternate raw
   visibility and cost, not as proof of compressed preservation.
4. Retain authored/parsed/effective distinctions and all raw-fallback, rejected-
   output, discarded-range, missing-capture, and warm-repeat observations.
5. Keep size/charge and application outcome distinct from semantic acceptance.

Situation controls assert invalid input or missing evidence, never false credit.
All exercise remains **not yet**. Observation schema and assembly belong to the
separate evaluation-command owner, not a second report system in this part.

## Investigation log

### Q: What qualification receipt can the per-invocation check consume?
- Sources examined: fixture backend, validator, and evaluation's qualification record.
- Findings: completion emits no usable segment. Evaluation owns initial script
  qualification; delivery owns the later invocation-specific join.
- Missing evidence: an implemented identity-bound qualification receipt.
- Conclusion: unresolved, needs the evaluation-owned U3 witness and receipt.
  Preserve U1 -> U2 -> U3; do not replace the missing dependency with a second gate.

### Q: Does final-array acceptance prove the complete provider request is bounded?
- Sources examined: application call and `validateInvocation` above.
- Findings: it is heuristic array accounting with shrinking/unknown exceptions;
  captured system and tool fields still need separate size reporting.
- Missing evidence: matched actual provider captures and their measured costs.
- Conclusion: resolved with answer: preserve existing gates and report their scope.
