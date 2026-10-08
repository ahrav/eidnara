# cf-complete-independent-comparison

## Discovery trigger

U4 and the acceptance gates in the
[settled contract](https://github.com/ahrav/eidnara/issues/707), "Milestone
boundaries and dependencies" and "Evidence and review gate" (historical plan
lines 237-242,263-271), require complete attempts, stable comparison identity,
and human judgment independent of generated text or an optional model judge.
The desired baseline outcome cannot be replaced by its observed failure.

Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records plan
drift. Reachability is test-only: comparison consumes external evaluation
artifacts, not production transforms. Exercise: not yet.

## Evidence trail

- Plan line 239 rejects missing scenarios, digest/hash inconsistency, unreviewed
  judgments, model errors, and unresolved disagreement. Prompt hashes may differ.
- Plan line 240 requires identical corpus/scenarios and all configured attempts,
  including retries/fallbacks, with no best-of-N selection. Lines 269-270 separate
  desired outcomes, observed failures, reviewer authority, and model suggestions.
- Plan line 270 requires two people for the initial corpus and semantic baseline,
  a non-author reviewer for later changed judgments, and another human
  adjudication for disputed or severe findings. Annotation content belongs to U1.
- [Scripted model assignment](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L428-L429)
  accepts a script's model string. A real-model label therefore needs recorded
  execution mode and actual dispatch/response evidence, not that field alone.
- [Incident outcome types](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L33-L84)
  separate run health, behavioral verdict, and baseline comparison. The executable
  result lane is only `green`, despite the broader comparison vocabulary.
- [Outcome validation](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L207-L263)
  prevents unhealthy runs from carrying scored verdicts. [Completeness](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L380-L448)
  requires selected terminal rows and regards both evaluated pass and evaluated
  assertion failure as evaluation-complete.
- [Report tests](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L1015-L1037)
  reject missing/duplicate/unselected results. Tests at lines 1096-1195 separate
  unhealthy, blocked, regression, and expected-green cases. Status: `unaudited`.
- [Callback failure retention](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L1197-L1212)
  checks that one thrown callback leaves a static unevaluated crash row while the
  report still includes the selected set. This is incident prior art only.
- [Incident exit policy](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L672-L710)
  has dependency and baseline rules, not the fidelity gate. Local commit
  `de84c0d0` removes `known-red` from result lanes; current lines 65-66 confirm it.
- [Package scripts](../../../../../packages/e2e-tests/package.json#L6-L26) and
  filesystem inventory show no fidelity comparison command or review checks.

## Failure scenario

A candidate is retried until it succeeds and only the best answer is retained.
Its comparison omits the initial false transition and provider failure. A model
judge approves the surviving answer, or a baseline is regenerated to encode the
old failure as expected meaning. The final table looks complete and inexpensive.

The competing explanation is that existing incident result types prevent this.
They check selected incident rows, not fidelity attempts, semantic review, or
the independent source obligations. Their implementation cannot establish the
planned comparison policy. No actual evaluator best-of-N defect is alleged.

## Timing windows and dependencies

Freeze identities and limits before attempts start. Preserve failed sends and
fallbacks before later success is available. Complete review follows capture,
and any altered judgment needs the appropriate independent human approval.
Prompt changes are a treatment, not corpus identity drift.
Scripted and forward modes remain exclusive with no silent matcher/default
fallback. Initial U3 qualification follows U1/U2; each compared invocation still
needs its own delivery/recovery observations.

Keep execution, deterministic result, preservation, recovery, consumer safety,
semantic review, and cost separate. Invalid XML, discarded coverage, skipped
setup, missing capture, raw pass-through, and failed recovery remain distinct.
Unavailable evidence is not recovered evidence; safe abstention is not useful
retention. An observed baseline failure still blocks fidelity acceptance.
The [control-discrimination record](cf-semantic-control-discrimination.md) checks
recorded verdicts against sealed labels, not semantic truth. The
[cost-completeness record](cf-evaluation-cost-completeness.md) accounts for every
attempt, including unknown cost. Both feed comparison acceptance.

## What a test must construct

1. Reconcile each configured case/scenario/attempt with one terminal outcome and
   retain all actual retry/fallback requests and responses beneath that attempt.
   Missing responses remain unknown/error outcomes, not zero-cost successes.
2. Remove or duplicate an attempt, change only one side's scenario identity, and
   replace baseline obligations with candidate wording. Acceptance must fail.
3. Supply reviewed and unreviewed outputs, a model-only judgment, and an unresolved
   human dispute. Apply the plan's review gate without inventing a scoring model.
4. Include a structurally accepted real output with reversed meaning and an
   acceptable paraphrase. A structural pass alone cannot decide either meaning.
5. Show token savings beside a required fidelity failure. Savings cannot change
   that result to accepted.
6. Give a scripted output a real model string. Without real-mode dispatch and
   response evidence, `cf-eval-mode-origin-consistent` must reject real labeling.

## Investigation log

### Q: Can the incident report be reused as the fidelity acceptance engine?
- Sources examined: `report.ts:33-84,207-263,304-448,672-710`, related tests,
  and local `de84c0d0` report diff.
- Findings: Outcome distinctions are useful, but incident IDs, digest domains,
  green-only executable lanes, and dependency policy are specific to that pool.
- Missing evidence: No implemented fidelity attempt/review completeness check.
- Conclusion: Resolved with answer. Reuse applicable concepts/types, not the
  incident report contract or invented incident IDs. Related checks are unaudited.

### Q: How are human approvals and the first comparison bound to artifacts?
- Sources examined: Plan lines 134,239-242,270-282 and absent evaluator inventory.
- Findings: The plan fixes human authority and predeclared limits, but it does not
  supply a review artifact representation, named provider/model, or limit values.
- Missing evidence: Artifact-bound review records and an authorized run config.
- Conclusion: Unresolved, needs U4 review binding; provider/model/settings and
  limit selection need human input. No live call is authorized by this catalog.
