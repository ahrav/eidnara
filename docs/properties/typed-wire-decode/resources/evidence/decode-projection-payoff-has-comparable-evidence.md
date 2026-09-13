# decode-projection-payoff-has-comparable-evidence

System: plan-specific measurement evidence, test-only.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](../source-register.md) defines P and B. No benchmark runs.

Record status: invalidated for category mismatch after independent portfolio
review. The exact accepted obligation lives in
[EG1](../evidence-gates.md#eg1-decode-projection-payoff). This evidence file
retains the discovery trail; invalidation does not remove the plan's stop.

## Discovery trigger

P:L18 stops the whole plan when U1 payoff is within noise. P:L213 asks for
40/200-message decode plus projection, with before and after evidence under
one W1 marker. The existing W1 record is explicitly invalidated.

## Evidence trail

1. `docs/properties/hot-path-optimization/latency-audit/catalog.md:125-130`
   says each optimization owns its payoff and no catalog record owns a
   before/after measurement artifact. `:1772-1773` invalidates W1.
2. W1's retained text at `:1779-1795` asks for one marker per authorized
   stage and handler-level production shape at 1,400 or 2,500 messages.
   That text is historical, not an active obligation quietly restored here.
3. `crates/daemon/benches/hot_path.rs:1-10` describes local service-time
   experiments, per-cell reporting, and process-level replication.
4. `:69-80` prepares retained-original messages before timing; `:99-115`
   times projection alone. `:29-36` uses 100/1,400/2,500 projection counts,
   not the new 40/200 decode group.
5. `crates/daemon/Cargo.toml:77-81` requires `bench-internals` for hot_path.
   P:L214's command omits this feature and `--locked`.
6. P:L250-L267 reports debug allocation inventory, not before/after timing
   samples. The derive-only mirror is not the final optimized code.

The source register pins the actual baseline B. Local main lacks the meter;
the word `main` cannot identify a comparable before artifact in this checkout.

## Failure scenario

A projection-only bench is labeled decode-plus-projection, or one size wins
while the other is unrecorded. A single favorable run is treated as evidence
despite the benchmark's replication caveat. W1's old status is overwritten
to imply an approved handler-level measurement gate.

Competing explanation: a real speedup exists but the artifact is incomplete.
Preserve that as unverified. Neither an incomplete manifest nor a reported
allocation reduction supplies a timing result.

## Timing windows and dependencies

The timed function performs production `from_slice::<TransformRequest>` and
`wire::project_messages` from the same immutable body. It keeps both outputs
live through the declared endpoint and fixes whether destruction is timed.
An already decoded fixture cannot stand in for this operation.

Record separate source and binary identities for before and after, with the
same build configuration, locked dependencies, allocator, host, corpus,
warmup, repetition, and order policy. Record exclusions: no transport,
request probe, admission gate, store work, or response encoding is timed by
this narrow operation unless explicitly added under a different label.

## What a test must construct

For this invalidated record, the construction belongs to an evidence-gate
execution rather than a runtime test or coverage claim:

- Two immutable corpora, exactly 40 and 200 mixed messages, with hashes and
  block/payload distributions, not only message counts.
- Both source artifacts and actual benchmark binaries with feature manifests.
- Four required cells: each size before and after. A fixed composite marker
  fires only when all four exist, not when any cell completes.
- Raw process-level evidence, declared summary and uncertainty calculation,
  and a gain/noise rule fixed before observing candidate results.
- A `stop` or `blocked` verdict if evidence is incomplete or gain does not
  clear that rule. Do not search repeatedly until a favorable sample appears.

## Investigation log

### Q: Can this catalog reactivate W1 by naming it?

- Sources examined: W1 catalog status, historical evidence, and P:L212-L214.
- Findings: The plan and W1 ownership statement conflict. Historical W1 scope
  also differs from this narrow operation experiment.
- Missing evidence: An owner decision about the requested W1 update.
- Conclusion: needs human input. W1 remains invalidated; keep plan-local
  evidence separately. EG1 carries the obligation; R6 is not active.

### Q: What exactly counts as within noise?

- Sources examined: P:L18, P:L213; benchmark header and reported inventory.
- Findings: The stop rule exists but no estimator, independent replication
  count, uncertainty rule, or joint decision for both sizes is supplied.
- Missing evidence: A predeclared measurement and acceptance contract.
- Conclusion: needs human input. No percentage threshold is invented here.

### Q: Why invalidate R6 while retaining the payoff obligation?

- Sources examined: P:L18, P:L213; W1 status; independent finding 2.
- Findings: A comparable-evidence requirement governs an acceptance decision,
  not runtime safety, liveness, or a reachable system state.
- Missing evidence: Prospective before/after records and the noise rule.
- Conclusion: resolved on category. Keep R6 and its evidence for traceability;
  route EG1 through statistics, experiment design, and bench comparison.
  W1 remains invalidated, and within-noise payoff still stops the whole plan.
