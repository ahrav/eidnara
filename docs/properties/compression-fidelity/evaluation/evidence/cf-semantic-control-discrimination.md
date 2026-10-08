# cf-semantic-control-discrimination

## Discovery trigger

The fresh review identifies a missing safety check for U4 scenario 2 in the
original plan (historical line 238). The
[settled contract](https://github.com/ahrav/eidnara/issues/707) retains this
obligation in "Evidence and review gate" and "Provenance and reusable catalog".
The evaluator must distinguish known-bad samples from acceptable
paraphrases and a successful-deployment control. Merely presenting those samples
or recording a verdict for each does not establish discrimination.

Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records the original
plan and revision drift. Reachability is test-only: the planned evaluator is
absent at HEAD. Exercise: not yet. The comparison below is a claim under test,
not an executing semantic judge or a new control schema.

## Evidence trail

- Plan line 238 names reversed negation, planned-to-completed, inferred-to-observed,
  wrong source identity, and a hint with a lost qualifier as bad samples. It also
  requires acceptable paraphrases and the successful-deployment control.
- Plan lines 54-58 and 158 keep human-authored obligations independent of generated
  wording and any model judge. Lines 189-195 separate candidate output from the
  immutable oracle and exclude answer keys from provider input.
- Plan lines 239 and 266-270 reject unreviewed or disputed acceptance and preserve
  human authority. Control expectations cannot be regenerated from the verdicts
  they are meant to challenge.
- [Source-obligation independence](../../generation/catalog.md#cf-source-obligation-independence)
  owns annotation independence. This record consumes those labels. “Sealed” means
  fixed independently before candidate output and unchanged during comparison;
  it does not introduce a signing service, database, or new file format.
- [Incident outcome validation](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L207-L302)
  checks valid health/verdict/baseline combinations, not correctness against the
  fidelity control labels.
- [Incident completeness checks](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L396-L448)
  reject missing or duplicate selected rows. Their
  [tests](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L1015-L1037)
  are `unaudited` prior art for set completeness, not semantic discrimination.
- [Package scripts](../../../../../packages/e2e-tests/package.json#L6-L26) and
  filesystem inspection show no fidelity evaluator or sealed-control comparator.

## Failure scenario

An evaluator records “acceptable” for every sample, including reversed negation.
Another evaluator always abstains or rejects, including the confirmed deployment
and acceptable paraphrases. Both produce complete verdict lists. A presence-only
control marker lets either shortcut appear qualified.

The competing explanation is that human review alone makes a recorded-control
check unnecessary. Human judgment remains essential, but U4 also explicitly
requires these samples to discriminate. Matching recorded verdicts to independently
fixed labels catches missing, constant, or mismatched decisions without claiming
that deterministic code itself understands the sample's meaning.

## Timing windows and dependencies

U1 fixes source-backed labels before candidate output. U4 binds each recorded
control verdict to that label and its batch. Changing labels after inspecting
candidate output invalidates this comparison rather than correcting the oracle.
No new implementation-unit order is introduced.

This safety check gates “reviewable comparison,” not whether a human may inspect
a failed batch. Real batches with failures remain useful reachability evidence.
The check does not prove that labels or judgments are semantically correct.
Blinding comparison-arm labels is an optional review recommendation, not a
mandatory acceptance condition under the settled plan.

## What a test must construct

1. Supply the declared control IDs, their sealed expected labels, and a complete
   set of recorded verdicts. Compare IDs and verdicts deterministically.
2. Challenge all five known-bad classes and both positive-control kinds. Their
   separately named `cf-eval-control-*` occurrence markers are listed in the
   [fault map](../fault-map.md#named-safety-checks-and-coverage-checks-to-add).
3. Substitute always-accept and always-abstain/reject verdict sets. Both must
   fail `cf-eval-control-verdicts-match` on the relevant opposite controls.
4. Remove or duplicate a verdict or alter an expected label after sealing.
   The comparison cannot become reviewable through that mutation.
5. Keep semantic adjudication separate. Passing the recorded-verdict comparator
   never replaces source review or the required first real-output human review.

## Investigation log

### Q: Does existing report completeness establish control discrimination?
- Sources examined: Plan line 238, `report.ts:207-302,396-448`, and
  `runner.test.ts:1015-1037`.
- Findings: Existing checks establish incident row/outcome consistency. They
  neither contain the fidelity controls nor compare their semantic labels.
- Missing evidence: An implemented fidelity comparator and recorded control set.
- Conclusion: Resolved that completeness alone is insufficient; this is a
  separate safety surface. Existing related checks remain `unaudited`.

### Q: How will verdicts bind to sealed labels and a captured batch?
- Sources examined: Plan lines 158,189-195,238-240,270-271 and U1's property owner.
- Findings: The plan fixes label independence and artifact identity but supplies
  no executing evaluator or review-record representation at HEAD.
- Missing evidence: U4's implemented control identity and verdict binding.
- Conclusion: Unresolved, needs U4 implementation using U1 labels. No framework
  or mandatory arm-blinding rule is added by this record.
