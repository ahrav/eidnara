# cf-reviewed-semantic-batch-reached

## Discovery trigger

The [settled contract](https://github.com/ahrav/eidnara/issues/707), "Milestone
boundaries and dependencies" U4 and "Evidence and review gate", requires a first
real-output batch with human review (historical plan line 242). Missing provider
access or review leaves semantic evidence unverified and acceptance withheld.
U3's missing fold scenarios remain blockers even when other rows can run
(historical plan line 232). Optional execution is not optional evidence for
full acceptance.

Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records the
original plan and HEAD drift. Reachability is test-only: this is a proposed
evaluation path absent at HEAD. Plan line 134 makes future live capture opt-in
with a named provider/model; that is not an existing configured path. Exercise:
not yet.

## Evidence trail

- Plan line 93 requires all six cases to receive P1-P4 serving coverage and at
  least one pressure/omission scenario. U2/U3 own what those scenarios mean.
- Plan lines 233-235 propose an ignored producer entrypoint and a bounded
  record-and-forward consumer loop. They do not establish running capabilities.
- Plan lines 237-241 distinguish scripted outputs from real model evidence and
  require known-bad samples, acceptable paraphrases, and a successful-deployment
  control. These controls are review inputs, not generated answer keys.
- Plan lines 266-270 block acceptance for missing model/scenario/review evidence
  and define human authority. Lines 318-319 separate harness completion, safe
  behavior with missing evidence, and successful preservation/recovery.
- [Mock provider options](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L82-L134)
  expose a local server, not forwarding. Its response path at lines 192-224
  selects scripts or defaults, not actual remote model output.
- [Harness prompt selection](../../../../../packages/e2e-tests/src/rust-harness.ts#L375-L386)
  fixes `mock-anthropic/mock-sonnet`. That is not a named real comparison model.
- [Producer E2E assertions](../../../../../packages/e2e-tests/tests/rust-history_summarizer-producer.test.ts#L82-L90)
  check backend counters. [Fold execution](../../../../../packages/e2e-tests/tests/rust-fold-under-pressure.test.ts#L15-L42)
  needs an optional flag. Neither is the required complete semantic batch.
- [Whole-pool report test](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L1214-L1229)
  uses fake-child envelopes. Evaluation-complete there is not human-reviewed
  compression evidence. Status: `unaudited`.
- [Current package scripts](../../../../../packages/e2e-tests/package.json#L6-L26)
  have no evaluation command. [CI](../../../../../.github/workflows/ci.yml#L977-L1004)
  invokes fixture and Rust E2E checks, not real semantic capture or review.

## Failure scenario

Every deterministic assertion passes, optional live capture is disabled, and a
report calls the system semantically verified. Alternatively, a few real rows
run while required fold rows skip, or a real batch remains unreviewed. These
are missing evidence, not successes of the optional-mode safety property.

The competing explanation is that a required “all reviewed answers pass” marker
would enforce reachability. It would instead conflate test execution with
fidelity correctness. A reviewed real batch with false transitions must fire
the reachability marker and still fail the independent acceptance check.

## Timing windows and dependencies

Initial U3 qualification follows U1/U2 and precedes complete real capture. The
[qualification record](cf-fixture-script-qualification.md) owns that initial
gate; delivery and recovery still require observations for every credited
invocation. This does not reorder the plan's implementation units. Human review
must refer to the captured batch, not a later regenerated answer. Provider
access, synthetic data authorization, and review are explicit prerequisites.
No unbounded eventual-success claim is made for unavailable prerequisites.

The campaign marker is `sometimes`: it asserts a meaningful situation, not a
line in an ignored test or entry into a CLI. An unfired marker can mean absent
workload/access/review or genuinely unreachable required behavior. Neither is
formal liveness evidence. Both leave full acceptance withheld.

## What a test must construct

1. For an authorized acceptance campaign, retain a real-output batch covering
   the required cases/scenarios and the U2/U3 publication/provider observations.
2. Complete required consumer tool loops and retain the corresponding results,
   rather than attaching scripted answers to a “real model” label.
3. Obtain the plan-required human review using source spans, final invocation,
   candidate answer, and alleged violation. Record failures as well as passes.
4. Contrast with offline-only, partially reached, and unreviewed batches. Those
   cannot satisfy `cf-eval-reviewed-real-batch`.
5. Include known-bad and positive controls so universal rejection or abstention
   does not masquerade as discrimination. Their recorded verdicts must meet
   [cf-semantic-control-discrimination](cf-semantic-control-discrimination.md)
   for a reviewable comparison; that deterministic check is not semantic proof.

## Investigation log

### Q: Does any current CI lane establish this batch?
- Sources examined: Current CI, package scripts, fixture/fold tests, and
  incident fake-child report test at the anchors above.
- Findings: These are deterministic or gated checks. No real fidelity capture
  command, retained batch, or human review is supplied at HEAD.
- Missing evidence: The implemented, authorized, reviewed real-output batch.
- Conclusion: Resolved that existing selection is not this evidence; unresolved
  until U4 runs. Historical evaluator inventories do not change the conclusion.

### Q: Who supplies access/review, and can every required fold scenario run?
- Sources examined: Plan lines 134,219-227,232-242,270 and the current fold gate.
- Findings: The plan requires a named provider/model and two-person initial
  review. It explicitly treats fold qualification as an implementation dependency.
- Missing evidence: Authorized provider access, reviewers, and ungated U3 captures.
- Conclusion: Access and reviewer assignment need human input; fold reachability
  needs U3 execution. Do not resolve either by skipping required rows.
