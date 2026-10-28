# future-correction-keeps-kernel-maturity-and-retrieval

## Discovery trigger

#597 acceptance: create admits `ModelInference` with `Other`, and the actual
disposition, visibility, and maturity are verified even when the outcome word
is `deny`; revise creates a fresh successor that preserves domain and source
identity, increases the source revision, inherits no trusted maturity, and
never folds into a survivor; explicit retrieval can see the successor while
automatic retrieval respects the candidate ceiling.

A review of the create and revise change found that the support of an accepted
decision is cited only through decision events, which the served read did not
fold, so a support row tightened to `Secret` after acceptance left the
decision served at `Sensitive`.

## Evidence trail

`crates/kernel/src/review_acceptance.rs` - `revise` derives a fresh object id
from the settlement key, refuses one already in the registry (`fresh`), and
writes the successor through `Envelope::supersede_decision`; `admit` cites the
support through decision events and records the admission, whose
`AdmissionDecision` becomes the settlement's `AdmittedState`.

`crates/kernel/src/admission.rs` - `visibility_row` maps an active candidate to
`ExplicitLabeled`; `Automatic` needs `Verified` or above.

`crates/kernel/src/admission.rs:3509` - `strictest_support_evidence_sql` picks
the strictest evidence row the decision's `memory_reviewer_support` events cite
at the snapshot. The served query joins that row ahead of the own-history
`CROSS JOIN`, and `LatestRow::read` (`admission.rs:3836`) folds its live class
into the shared sensitivity beside the directly cited evidence.

`crates/kernel/src/cas/ingest.rs:1330` - a classification merge writes the
merged class onto every `evidence_meta` row of the digest without touching the
decisions that cite them, so serving must read the class live.

## Failure scenario

The successor reuses a live object id and folds the predecessor into it, or is
admitted with the predecessor's explicit-user maturity and surfaces
automatically. Or a support row is reclassified `Secret` after acceptance and
explicit search still serves the model text written from it.

## Timing windows and dependencies

The tightening lands in a later commit than the acceptance. Serving reads the
evidence class at read time, so no window remains once the merge commits.

## What a test must construct

An explicit-user decision revised through an accepted proposal; a staged
subject created through one. Read each new decision's registry row, its stored
admission, and its surface eligibility for explicit search and auto-inject.
Then re-ingest the bytes of one support row asserted `Secret`, choosing a row
other than the first cited one, and assert explicit search refuses the
decision.

The served-read cost of the support fold must not grow with the object's own
admission history: `support_citation_cost_does_not_scale_with_own_history` in
`crates/kernel/src/admission.rs` compares 200 support citations beside one and
41 own-history rows.

## Investigation log

### Q: Does the successor's admission read the predecessor's history?

- Sources examined: `crates/kernel/src/admission.rs` (`load_prior_decision`).
- Findings: the revise test asserts `candidate` for the successor of an
  explicit-user decision, so no prior maturity reaches it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does a support row tightened after acceptance restrict the decision?

- Sources examined: `crates/kernel/src/admission.rs` (`served_classes_sql`,
  `LatestRow::read`), `crates/kernel/src/cas/ingest.rs`
  (`MergedClassification::apply`).
- Findings: before the fold, serving read only the decision's direct
  `evidence_id` and its triggers; an accepted decision has neither, so the
  tightening never reached it.
  `an_accepted_decision_serves_no_lower_than_the_support_it_cites_reads_today`
  failed before the fold and passes after it.
- Missing evidence: none.
- Conclusion: resolved with answer.
