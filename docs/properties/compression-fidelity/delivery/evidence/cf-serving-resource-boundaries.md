# cf-serving-resource-boundaries

## Discovery trigger

R9 and the supplied plan's performance guardrails require fidelity evaluation
to stay outside production serving. Resource, dependency, replay, and review
passes identify a separate obligation from semantic quality and cost reporting.
Inspected 2026-09-19 at `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The plan's hard-budget requirement remains binding; observed implementation
details do not authorize a weaker acceptance threshold.

## Evidence trail

- [decay_render.rs:541-583](../../../../../crates/daemon/src/decay_render.rs#L541-L583)
  returns the unwrapped history body. Its positive-budget guard demotes
  oldest-first, bounded by `history_segments.len() * 5`, until the body fits
  its effective budget or is empty. This is the absolute body-budget boundary.
- [memory_render.rs:191-198](../../../../../crates/daemon/src/memory_render.rs#L191-L198)
  divides the requested budget by the pressure multiplier before calling the
  renderer, then adds the session-history wrapper or its empty placeholder.
  [memory_render.rs:7-12](../../../../../crates/daemon/src/memory_render.rs#L7-L12)
  keeps the empty wrapper deliberately for prompt-cache stability.
- [m0_compose.rs:131-168](../../../../../crates/daemon/src/m0_compose.rs#L131-L168)
  measures that wrapped history slice. Above `requested_budget * 1.05`, it
  increases pressure by 1.15 and retries at most three times. It returns the
  last render even if the trigger remains true. The trigger is not a cap.
- [transform.rs:109-114,8190-8237](../../../../../crates/daemon/src/transform.rs#L109-L114)
  defines the hint limits; its
  [eligibility/query gate](../../../../../crates/daemon/src/transform.rs#L8190-L8237)
  avoids queries for already decided, ineligible, short, or augmented input.
- [transform.rs:8339-8392](../../../../../crates/daemon/src/transform.rs#L8339-L8392)
  enforces matched-term/rareness admission, the top-score threshold, and the
  result limit. Fidelity wording changes do not authorize broader searches.
- [lib.rs:9257-9271](../../../../../crates/daemon/src/lib.rs#L9257-L9271)
  pins one canonical read for the pass. The
  [reader](../../../../../crates/daemon/src/canonical_memory.rs#L147-L211)
  applies freshness, scope, category, and budget selection before rendering.
- [transform.rs:22965-22984,21111-21216](../../../../../crates/daemon/src/transform.rs#L22965-L22984)
  checks empty-hint query reuse; the
  [SOFT/replay test](../../../../../crates/daemon/src/transform.rs#L21111-L21216)
  checks frozen m0/m1 bytes. Both are unaudited component checks.
- [transform-session-client.ts:72,1552-1556](../../../../../packages/opencode-plugin/src/hooks/context/transform-session-client.ts#L72) with the [adapter profile](../../../../../packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts#L351)
  sets headroom to 250 permille; the
  [final application check](../../../../../packages/opencode-plugin/src/hooks/context/transform-session-client.ts#L1552-L1556)
  passes it to [charge/validate](../../../../../packages/opencode-plugin/src/hooks/context/invocation-budget.ts#L44-L71).
  The heuristic sums entry bytes, estimates once, and rounds the 25% uplift up.
  A known-limit candidate declines only when over limit and growing in bytes.
  `limit_unknown` and `shrinks` explicitly accept; this is existing policy, not
  an absolute cap over the complete provider request.

Reachability is **default-production**: normal composition uses the renderer,
hint gate, and pinned reader above. The record constrains production changes;
its source audit and matched replay run offline, not once per live transform.

## Failure scenario

A fidelity fix keeps more meaning by raising the history budget, querying again
on warm repeats, rereading native sources, or invoking a semantic judge during
transform. Another change admits memory that the original surface would exclude.
The resulting answer can improve while violating R9's production boundary.

A different false pass applies `body_tokens <= 1.05 * budget` because the m0
retry loop uses 105%. That weakens the positive-budget body guard. Conversely,
asserting a new strict whole-provider cap invents a contract the array gate does
not enforce. The three code-defined scopes resolve that apparent conflict
without changing the plan or packing policy.

## Timing windows and dependencies

Compare the same configuration, estimator, source/read state, and serving inputs
before and after the candidate change. Check cold composition and warm reuse
separately. Text changes can change charges and demotion under an unchanged
budget; this record does not require different texts to render identical bytes.
Within each run, unchanged repeated inputs must retain the existing cache policy.
Measure unwrapped body and wrapped slice independently; token estimates are not
assumed additive. Do not combine Rust counts with the TypeScript heuristic.
The evaluation owner stores cost records; this part supplies boundary observations
once and does not create a second cost model or performance threshold.

## What a test must construct

1. Audit production call sites for feature-added per-transform semantic judging,
   native-source rereads, and storage queries. Require zero such additions;
   retain existing summarizer, hint, and canonical-memory behavior.
2. With a positive effective history budget and the production estimator,
   require an empty body or `estimate_tokens(body) <= effective_history_budget`.
   Reuse existing guard/golden checks, including forced high-importance pressure.
3. Observe wrapped-history charge and the separate initial render plus at most
   three retries. Do not turn its trigger or final result into a 105% allowance.
4. Exercise query suppression/reuse, existing candidate/result/fragment limits,
   pinned memory admission/exclusion, and cold/warm cache behavior with fixed
   inputs. Reuse the delivery situation markers rather than create a new harness.
5. Retain the plugin's 25%-headroom charge and known-limit over-limit/growth
   rejection, including `limit_unknown` and `shrinks` acceptance. Capture the
   measured full provider invocation and raw-fallback outcome separately;
   neither a fitting body nor an accepted array proves complete-context size.

No production callback, policy flag, API, or instrumentation service is needed.
All exercise remains **not yet**; existing checks remain **unaudited**.

## Investigation log

### Q: Does the m0 retry threshold permit every output to exceed budget by 5%?
- Sources examined: body guard, wrapper construction, and retry loop above.
- Findings: the body uses its effective hard budget. The wrapper has a separate
  105% trigger and bounded retries, with no guaranteed 105% postcondition. The
  OpenCode array gate has separate heuristic and exception rules.
- Missing evidence: matched execution and full-invocation measurements, not a
  static budget-scope decision.
- Conclusion: resolved with answer: preserve the hard unwrapped-body budget and
  both existing policies. No global 105% allowance or whole-context cap follows.

### Q: Does recording cost prove no forbidden production work was added?
- Sources examined: plan R9 and performance guardrails, source boundaries above.
- Findings: cost accounting and serving-work restrictions are separate claims.
- Missing evidence: a candidate change and its source audit/matched replay.
- Conclusion: resolved with answer: cost alone does not prove compliance;
  execution remains unverified. Evaluation owns the cost record, and lower cost
  cannot excuse a forbidden per-transform operation.
