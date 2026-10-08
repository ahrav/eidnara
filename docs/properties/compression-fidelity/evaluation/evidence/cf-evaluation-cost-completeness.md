# cf-evaluation-cost-completeness

## Discovery trigger

The fresh review identifies cost completeness as a separate safety surface in
the [settled contract](https://github.com/ahrav/eidnara/issues/707), "Evidence
and review gate" and "Resource and security invariants" (historical plan lines
271,278-285). Enforcing a provider cap alone does not prove that reported
serving, generation, transform, and recovery costs are complete or comparable.

Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records the plan
and revision drift. Reachability is test-only: U4's evaluation command is absent
at HEAD. Future real capture is opt-in, not an existing production path.
Exercise: not yet. No provider calls, measurements, or cost limits are invented.

## Evidence trail

- Plan line 271 requires costs to derive from owning observations and retains
  every declared attempt. Line 240 includes retries and fallbacks, not best-of-N.
- Plan line 278 requires bytes/tokens with estimator/model identity, generation
  input/output and attempt count, transform time, and recovery call/output cost.
- Plan line 279 separates cold rematerialization from warm repeated serving and
  includes repeated-source leakage. A normalized last-request size is not all
  of this evidence.
- Plan lines 281-282 preserve serving budgets and require call count, maximum
  output, timeout, and total provider-spend cap frozen before comparison. Failures
  cannot extend them. Lines 283-285 supply no latency baseline or percentage
  threshold and give fidelity blockers precedence over savings.
- [Last-request size helpers](../../../../../packages/e2e-tests/src/rust-harness.ts#L444-L453)
  serialize the last main request's messages; missing capture returns zero/empty
  output. They do not account for all requests, system/tools, or actual charges.
- [Scripted usage guard](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L287-L294)
  requires usage unless the error path was selected. These are supplied mock
  counters, not measured remote-provider consumption. Status: `unaudited`.
- [Mock model configuration](../../../../../packages/e2e-tests/src/opencode-runner/spawn.ts#L174-L187)
  uses zero prices. It cannot support a real generation/recovery cost claim.
- [Prompt timeout](../../../../../packages/e2e-tests/src/rust-harness.ts#L375-L406)
  races a timeout with the prompt promise. Losing the response does not establish
  that provider work or billing stopped.
- [DiagnosticSink](../../../../../packages/e2e-tests/src/incident-pool/support/case-workspace.ts#L129-L159)
  is bounded private diagnostic prior art, not a provider-cost limiter. Its
  [tests](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L925-L979)
  remain `unaudited`. No complete fidelity cost check was found.

## Failure scenario

A report keeps only the final warm successful request. It omits cold transform
work, leaked raw source, a failed producer attempt, and a recovery tool exchange.
Its budget field is unchanged, but its observed cost is understated. A lost
provider response is reported as zero charge, or a retry gets a fresh allowance.

The competing explanation is that token counts plus a fixed spend cap suffice.
The plan explicitly requires separate phases and every attempt. Mock usage and
zero model prices distinguish existing harness controls from measured cost.
The property does not assert that an unknown charge can be calculated exactly.

## Timing windows and dependencies

Declare limits before execution and bind them once in the authoritative manifest.
Retain every attempted dispatch and distinguish acknowledged usage from unknown
outcomes. Record known actual costs and unresolved usage/cost separately; unknown
does not mean zero. Missing required evidence withholds cost-complete status.

[Authoritative assembly](cf-authoritative-evidence-assembly.md) owns observations;
this record owns completeness and phase/identity checks over them. The
[forwarding record](cf-bounded-record-and-forward.md) consumes these unchanged
whole-loop limits. Its optional enabled mode cannot waive default offline safety.
No new budget values, pricing service, latency baseline, or performance threshold
is introduced. This is U4 work after the required U2/U3 evidence, not a new unit.

## What a test must construct

1. Retain serving bytes/tokens with estimator/model identity, generation input/
   output and attempts, transform time, and recovery calls/output for each run.
2. Include cold rematerialization, warm repeats, repeated raw source, retries,
   fallbacks, and a recovery exchange; keep each observation separately attributed.
3. Drop one metric or its identity, omit an attempt, pool cold/warm values, or
   remove raw-source cost. `cf-eval-cost-observations-complete` must fail.
4. Lose a provider response after send. Keep the attempted call and unknown
   usage/cost; do not fabricate zero cost or silently grant another allowance.
5. Reach each frozen call/output/time/spend boundary and propose a limit increase
   after failure. The limits remain unchanged and execution stops at its bound.
6. Compare fidelity and cost separately. Lower measured cost cannot erase a
   failed obligation, and incomplete cost cannot be labeled a complete saving.

## Investigation log

### Q: Which existing limiter or size helper can be reused?
- Sources examined: `rust-harness.ts:444-453`, `case-workspace.ts:129-159`, and
  `runner.test.ts:925-979`.
- Findings: Size helpers observe a subset of one request. DiagnosticSink bounds
  private diagnostics but neither records every cost nor limits provider spend.
- Missing evidence: U4's complete per-attempt observations and limit accounting.
- Conclusion: Resolved with reuse boundaries. Reuse bounded private diagnostics
  where applicable; do not claim an existing complete cost evaluator.

### Q: How are ambiguous costs and the first run's limits determined?
- Sources examined: Plan lines 235,240,271,278-283; mock usage/prices; prompt timeout.
- Findings: Limits must be frozen and attempts retained, but neither actual
  provider usage/pricing inputs nor an accounting implementation is supplied.
- Missing evidence: Attribution of known costs and unresolved exposure, named
  estimator/model identities, and approved run-specific limits.
- Conclusion: Accounting needs implementation; provider/estimator selection and
  limit values need human input. No invented actual charge or threshold closes it.
