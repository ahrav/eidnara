# directive-revise-supersedes-same-lineage-and-reapproves

## Discovery trigger

Plan requirement R7 (correction by successor at `source_revision + 1`,
predecessor superseded, dependents lose support) and KTD3 (`supersede_decision`
then `AcceptedAdr` on the successor; succession cannot cross lineage or
class). U3 step 3 and the test scenario "`directive.revise` leaves the
predecessor `superseded`, the successor Approved, and demotes a row that
cited the predecessor as approval".

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/admission.rs:1312-1349` `supersede_decision` ->
  `supersede_decision_inner`: refuses a predecessor whose latest admission is
  Quarantined, Rejected, or Contradicted (`refuse_barred_lineage`), refuses a
  barred live fold target, then calls `correct_decision`.
- `crates/kernel/src/slice/write.rs:354-452` `correct_decision_inner`: samples
  `granted_before`, folds the predecessor's sensitivity into the replacement
  (`:374-378`), refuses a survivor equal to the predecessor (`:388-393`),
  validates the successor, invalidates the predecessor, inserts the
  replacement, sets `superseded_by` (`:420-425`), and when the predecessor
  granted authority calls `demote_dependents_if_authority_lost` (`:448-452`;
  the plan cites `:443-448`, which is the change push just above; the cascade
  call starts at `:448`).
- `write.rs:1271-1284` `validate_successor`: same `domain_id`, `source_kind`,
  `source_id`, and `source_revision > old` else `Conflict`. The code allows
  any greater revision; the plan fixes `+1`.
- `admission.rs:268-274`: a prior with different classes is `AdmissionPolicy`.
  The successor is a new object with no prior, so class continuity on revise
  is a route obligation, not a Kernel one; the predecessor's row is never
  re-admitted.
- `admission.rs:1566-1580`: `AcceptedAdr` does not inherit the stored approval;
  the successor self-approves from its own accepted status.
- `commit.rs:787-848` shows the daemon pattern for supersede with a revision
  check (`RevisionNotAdvanced`) and the sensitivity floor.
- Tests: `crates/kernel/tests/kernel_admission.rs:788`
  `correction_preserves_predecessor_history_and_emits_both_changes`; `:991`
  `superseding_a_quarantined_predecessor_is_rejected`; `:3221`
  `superseded_replacement_serves_only_after_its_own_decision`.

## Failure scenario

Legal witness: D1 at rev 1, Approved. M is a model row admitted with
`approval_object_id = D1` and elevated support. `revise(D1, T2)` commits one
envelope: `supersede_decision(D1, spec{rev 2, same lineage, Normal, T2})`
then `record_admission(AcceptedAdr, explicit_user/user_explicit, D2)`. D1 is
invalidated with `superseded_by = D2`; D2 is Approved and Visible; M's latest
admission is `ApprovalRevoked` (reason "authority corrected") at its ceiling.

Smallest violating traces:

1. The handler supersedes but forgets `AcceptedAdr` on D2: D2 is a live
   `adr_accepted` decision with no admission row; Hidden; the user's rule
   vanished while the UI reported success.
2. The handler admits D2 with `EventKind::Other` through the generic `admit`
   helper: `Other` cannot raise support; D2 is Candidate.
3. The handler builds the spec with `source_revision = 1` (unchanged): the
   Kernel returns `Conflict`; correct, but the route must map it to a
   refresh-and-retry outcome, not a defect.
4. The handler changes `source_id` to a new uuid: `validate_successor`
   returns `InvalidInput`; lineage continuity is lost if the handler then
   falls back to insert plus retire.

## Timing windows and dependencies

All in one envelope. The cascade is bounded by `MAX_AUTHORITY_DEMOTIONS`
(`admission.rs` constant) with deferral, never failure, so a revise over a
widely cited directive still commits.

## What a test must construct

- D1 recorded through the route; M seeded citing D1 (store API, as
  `kernel_admission.rs:469` does).
- `revise(D1, T2)`; assert D1 invalidated and `superseded_by = D2`; D2
  `source_revision = 2`, same `source_kind`/`source_id`; D2's admission
  Approved/automatic/explicit_user; M's latest admission `approval_revoked`
  with `effective_maturity <= candidate`.
- Negative: `revise` on a Rejected directive is refused (`AdmissionPolicy`).
- Negative: `revise` with a body carrying `source_revision` or a class field
  is `invalid_params`.

## Investigation log

### Q: Does the Kernel require exactly `+1`?
- Sources examined: `write.rs:1281-1283`.
- Findings: strictly greater only; `+1` is the plan's choice.
- Missing evidence: none.
- Conclusion: resolved with answer: Kernel allows any increase; the record
  checks `+1` as the plan states.

### Q: Does revise carry the predecessor's scope?
- Sources examined: plan U3 step 3 body `{object_id, text, operation_key}`.
- Findings: no scope field; `DecisionSpec.scope_id` must come from the
  predecessor's `ObjectState.scope_id`.
- Missing evidence: a design statement.
- Conclusion: needs human input (recorded in open-questions.md).
