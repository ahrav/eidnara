# cf-bounded-record-and-forward

## Discovery trigger

U4 in the [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Milestone boundaries and dependencies" and "Resource and security invariants"
(historical plan lines 234-241), specifies optional record-and-forward on the
existing Messages provider. It requires the selected real request, returned
response, and actual
registered-tool loop, with synthetic data, private artifacts, no credentials,
and fixed whole-run limits. These are claims under test, not implemented features.

Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records drift.
Reachability is test-only: the evaluation/forwarding path is absent at HEAD.
Plan lines 134,234-235 require future live mode to be opt-in. Exercise: not yet.
No external model call was made.

## Evidence trail

- [Provider options/start](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L82-L134)
  expose only a port and bind locally. [Dispatch](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L221-L302)
  parses requests, copies all headers, and chooses matcher/queue/default output.
  There is no forwarding configuration or outbound provider fetch in this file.
- [Model assignment](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L428-L429)
  lets a script supply the model string. The field alone cannot prove real origin.
- [Capture type](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L66-L80)
  retains no response body. [Response timestamp](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L286-L327)
  is assigned before response construction; it does not prove OpenCode received
  the response or completed a tool loop.
- [Scripted tool flow](../../../../../packages/e2e-tests/src/scripted-tool-call.ts#L93-L145)
  returns a tool call and finds its provider-visible result. This supplies local
  loop prior art, not model-selected arguments or real response forwarding.
- [Prompt sending](../../../../../packages/e2e-tests/src/rust-harness.ts#L375-L406)
  hardcodes `mock-anthropic/mock-sonnet` and races a timeout without establishing
  cancellation of provider work. Changing a model after capture would violate
  the plan's same-request requirement.
- [Mock configuration](../../../../../packages/e2e-tests/src/opencode-runner/spawn.ts#L174-L187)
  supplies a fake API key, zero prices, and mock output limits. Mock usage and
  zero prices cannot establish real generation or recovery cost.
- [Publisher](../../../../../packages/e2e-tests/src/atomic-publish.ts#L5-L16)
  defaults to mode 0644; U4 must pass explicit `{ mode: 0o600 }` and exclude
  credentials before writing. No such fidelity integration is implemented.
- [DiagnosticSink](../../../../../packages/e2e-tests/src/incident-pool/support/case-workspace.ts#L129-L159)
  provides capped private diagnostic writes. Reuse this prior art before adding
  another limiter; it neither redacts credentials nor proves capture completeness.
- [Incident isolation tests](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L804-L881)
  check credential stripping, non-loopback refusal, and unsafe environment
  overrides. Status: `unaudited`. Live evaluation is a separate command; these
  offline incident-runner guards must not be relaxed for it.
- Plan lines 278-283 require byte/token accounting with estimator/model identity,
  generation attempts, transform time, recovery calls/output, cold/warm separation,
  and limits frozen before comparison. No numeric values are invented here.

## Failure scenario

A “forwarded” sample is actually a scripted continuation, the model is replaced
after OpenCode constructs its request, or tool results are synthesized outside
the registered-tool loop. Separately, copying incoming headers leaks a real
credential, or each retry resets the call/time/spend allowance.

The competing explanation is that the current mock already supplies safe live
capture. Its options, request-only capture type, scripted dispatch, and hardcoded
model contradict that claim. These are reuse constraints, not evidence of a
deployed live-mode security incident.

## Timing windows and dependencies

Select the real compatible model before capture. Credentials enter only the
outbound request; capture/config/log artifacts and environment dumps must exclude
them. The explicit HTTPS target and reviewed synthetic input constrain admission.
Redirect behavior must preserve those constraints, not silently change the target.
Recorded script and forward modes are exclusive. Forward failure cannot silently
fall back to a matcher, script, or default while keeping a real-model label.

The loop consumes the fixed limits owned by
[cf-evaluation-cost-completeness](cf-evaluation-cost-completeness.md), including
retries and ambiguous attempts. Exhaustion, unsupported shape, and an unfinished
loop produce an unreviewable outcome. Truncated diagnostics are not full captures.

## What a test must construct

1. Require `always` check `cf-eval-offline-zero-outbound` on every default/scripted
   run and `sometimes` marker `cf-eval-offline-default-invoked`. Only enabled
   forwarding uses `always-or-unreached`; default safety is not optional.
2. With explicit forwarding enabled, bind each actual selected-model request to
   the returned response, then bind provider tool-call IDs to registered-tool
   results in subsequent captured requests through loop completion.
3. Use credential canaries and inspect captures, config, and logs for absence;
   observe outbound authentication separately and private JSON mode 0600.
4. Challenge each frozen limit across multiple turns and retries, plus provider
   errors, unsupported response shape, and missing tool results. No bound extends.
5. Keep usage, prices/estimator identity, serving cost, generation cost, and
   recovery cost separate from fidelity verdicts and from synthetic mock counts.
6. Challenge `cf-eval-mode-origin-consistent` with a forged model string and a
   silent scripted fallback. Neither can receive a real-model evidence label.

## Investigation log

### Q: How does the real selected model enter the captured request?
- Sources examined: Plan line 234, `server.ts:82-134,221-302`, and
  `rust-harness.ts:375-386`.
- Findings: The plan selects the real model before capture; current harness
  selection is hardcoded and no forwarding branch exists.
- Missing evidence: An implemented model-selection and same-request witness.
- Conclusion: Unresolved, needs U4 integration at the existing boundary. Do not
  invent a multi-provider adapter or relax incident isolation to make it work.

### Q: How will recorded mode prevent forged real-model evidence?
- Sources examined: Plan lines 234,237,241 and `server.ts:262-302,268-269`.
- Findings: Current scripts can override the model string; no forward-mode origin
  or exclusive-mode check exists. Incident isolation is a separate boundary.
- Missing evidence: Recorded mode bound to actual dispatch/response artifacts and
  explicit failure instead of silent script/default fallback.
- Conclusion: Unresolved, needs U4 origin checks. Cost accounting is owned by
  `cf-evaluation-cost-completeness`; neither safety check proves batch reachability.
