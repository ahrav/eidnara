# dispute-with-evidence-never-raises-standing

## Discovery trigger

Plan P4 KTD4 and index D3: the report adds data, not authority. Evidence
on a dispute must not become a lever on maturity or a path to a less
restrictive disposition.

## Evidence trail

- `event_effect` maps `MarkDisputed` to `(current, Some(Disputed), Deny)`
  and `Contradict` to `(current, Some(Contradicted), Deny)`
  (`crates/kernel/src/admission.rs:4161-4162`), so the target maturity is
  the current one.
- The only evaluator gate on evidence is `Enforce` without evidence
  (`admission.rs:275-277`); `prepare_with_facts` sets `has_evidence` from
  `evidence_id.is_some()` (`admission.rs:1650`), and the route's
  `evidence_ref` lives in `reason`, which no evaluator branch reads.
- `disposition_restrictiveness` orders the dispositions
  (`admission.rs:4168-4175`); a strict decrease without a valid approval
  is recorded denied with the prior kept (rule at `admission.rs:321-334`).
  A valid approval permits the relaxation: `denied` is
  `requires_approval && !approval_valid` (`admission.rs:330`), and
  `approval_valid` comes from `validate_approval` on the request's
  `approval_object_id` (`admission.rs:1587-1591`). The record's
  restrictiveness clause therefore holds unless the row names a valid
  approval, matching AC8 "no relaxation without a valid approval"
  (portfolio refinement R1).
- The route builds the request with `approval_object_id` from the body
  and evidence from the body (`crates/daemon/src/kernel_routes/commit.rs:722-734`);
  no evidence field reaches `trigger_object_id`, which stays `None`
  (`commit.rs:729`).

## Failure scenario

A dispute against a `Verified` decision carrying evidence E. If
`has_evidence` fed a promotion branch, or `event_effect` used a target
above `current`, the dispute would keep or raise the effective maturity
and the decision would keep serving on automatic surfaces.

## Timing windows and dependencies

None in time. A subject whose approval chain changes earlier in the same
envelope is judged after that change; the preview refuses that
combination separately (`commit.rs:1174-1180`).

## What a test must construct

All constructed:

- Every consistent `(historical, effective)` pair, every prior
  disposition, both dispute events, with and without evidence: `Deny`,
  historical kept, effective not raised, restrictiveness not lowered, the
  prior kept when the request would relax it
  (`a_dispute_with_evidence_never_raises_standing_across_the_grid`,
  `admission.rs:4532`; it counts `15 * |Disposition| * 2 * 2` cases).
- A dispute with evidence through the route reads `outcome = deny`,
  `maturity` equal to the prior, `effective_maturity = candidate`
  (`crates/daemon/tests/kernel_routes.rs:5723`).
- `rejected`, `contradicted`, `quarantined` priors: `denied: true`, prior
  kept, evidence on the row; `stale` prior: lands `disputed`
  (`a_dispute_is_denied_against_a_more_restrictive_prior_and_lateral_from_stale`,
  `kernel_routes.rs:6146`).
- A `contradicted` prior disputed with evidence: without an approval,
  `denied: true` and `contradicted` kept; with a valid
  `approval_object_id`, `denied: false`, `disputed`, `outcome = deny`, and
  the evidence id on the row
  (`a_dispute_relaxes_a_contradicted_prior_only_with_a_valid_approval`,
  `kernel_routes.rs:6418`). The grid test fixes `approval_valid: false`
  (`admission.rs:4557`), so this route test is the only one that
  constructs the approved relaxation.
- An approval above Candidate disputed through the route: historical
  maturity kept, effective maturity `candidate`
  (`kernel_routes.rs:6199`). The test asserts the prior's historical
  maturity is not `candidate`; it does not assert the prior's effective
  maturity.

## Investigation log

### Q: Should a dispute against a more restrictive prior be refused instead of recorded denied?

- Sources examined: `kernel_routes.rs:6146`; the specification's Q9 in
  [`../open-questions.md`](../open-questions.md).
- Findings: the route records the row with `outcome = deny`,
  `denied: true`, and the cited evidence id; the test pins that form.
- Missing evidence: an owner decision.
- Conclusion: needs human input.

### Q: Does the grid test construct effective maturity above Candidate?

- Sources examined: `admission.rs:4532-4593`.
- Findings: yes; it iterates every `Maturity` for both historical and
  effective with `effective.rank() <= historical.rank()`, which includes
  effective above Candidate.
- Missing evidence: none.
- Conclusion: resolved with answer.
