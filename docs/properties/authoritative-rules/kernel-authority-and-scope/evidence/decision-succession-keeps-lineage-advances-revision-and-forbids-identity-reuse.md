# decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse

## Discovery trigger

Plan KTD3, R7, R8, and the assumption "supersession cannot keep the
predecessor's `object_id` (`write.rs:388-393`); identity continuity is the
succession link". Directive revise and repository reconcile are both
successions, so the kernel's succession rules are the guarantee they lean on.

## Evidence trail

- `crates/kernel/src/admission.rs:1312-1337` `supersede_decision` and
  `_inner`: `load_subject_facts(old)` (live required), `refuse_barred_lineage`
  on `old`; when `replacement.object_id` is live, `refuse_barred_lineage` on
  the survivor; then `correct_decision`.
- `:1339-1349` `refuse_barred_lineage`: latest prior disposition in
  `Quarantined | Rejected | Contradicted` is `AdmissionPolicy`.
- `crates/kernel/src/slice/write.rs:362-456` `correct_decision_inner`:
  - `load_live_typed_object(old, "decision")` (`:368`);
  - `granted_before = subject_grants_authority(old)` (`:369`);
  - `replacement.sensitivity = fold_cited_evidence_class(..,
    replacement.sensitivity.restrictive(old.sensitivity))` (`:375-379`);
  - survivor lookup `load_live_decision_by_object(replacement.object_id)`
    (`:387`); `survivor.object_id == old.object_id` or a survivor classified
    below the correction is `InvalidInput` (`:389-393`);
  - `validate_successor` against the survivor or the spec (`:394-409`);
  - `invalidate(.., "decisions", old)` (`:410-415`); `insert_decision` only
    when no survivor (`:416-432`); `set_successor(old -> replacement)`
    (`:434-439`);
  - `demote_dependents_if_authority_lost` when `granted_before` (`:448-452`).
- `write.rs:1271-1285` `validate_successor`: `domain_id`, `source_kind`,
  `source_id` must equal `old`'s (`InvalidInput`); `source_revision <=
  old.source_revision` is `Conflict`.
- Route: `crates/daemon/src/kernel_routes/commit.rs:787-850`
  `SupersedeDecision`: predecessor must be in project (`:803`); a live
  replacement's stored revision is compared (`:811-832`);
  `successor_revision <= predecessor.source_revision` is
  `CommitFailure::RevisionNotAdvanced` (`:833-836`); successor sensitivity
  floor (`:838-841`); a fresh successor is admitted with `EventKind::Other`
  under the caller's classes (`:842-847`).
- Eligibility reports `Superseded` from `superseded_by` (`crates/kernel/src/eligibility.rs:209-210`).
- No `Disposition::Superseded` row is written: `EventKind::Correct | Replace`
  are emitted only by `supersede_domain` (`admission.rs:1252-1305`), and
  `refuse_succession_events` (`:2559-2567`) blocks callers.
- Tests: `crates/kernel/tests/kernel_slice.rs:1095` (fresh successor; fold
  writes no row; predecessor links to successor), `:1186`, `:1225`, `:1002`,
  `:1335`; `crates/kernel/tests/kernel_admission.rs:991`;
  `crates/daemon/tests/kernel_routes.rs:1066`, `:1426`.

## Failure scenario

A revise lands a successor on a different `source_id`, detaching the
directive from its lineage so a later revision cannot find it, or on the same
revision so two live rows share a lineage revision; a reconcile reuses the
predecessor's `object_id` so token holders see the old id survive with new
text; a rejected candidate is superseded back into service.

## Timing windows and dependencies

- Single transaction; a survivor fold keeps the survivor's stored row, so
  the successor spec's text is discarded when its id is already live.
- The kernel requires strictly greater revision only; the plan's `+1`
  convention is a route obligation (lead L1).
- The successor's admission classes are the caller's, not the predecessor's
  (lead L2).

## What a test must construct

1. Live decision `P` on `(domain d, kind k, id s, revision 2)`.
2. Specs varying one field: domain `d'` (`InvalidInput`), source kind `k'`
   (`InvalidInput`), source id `s'` (`InvalidInput`), revision 2
   (`Conflict`), revision 1 (`Conflict`), `object_id == P` with `P` live
   (`InvalidInput`), revision 7 (success; the kernel accepts any larger
   revision).
3. After success: `P.invalidated_commit_seq == commit`, `P.superseded_by ==
   Some(S)`, `S` live, `COUNT(admission_decisions WHERE subject = P AND
   commit_seq = commit) == 0`, and `S.sensitivity >= P.sensitivity`.
4. Barred: `P` with latest disposition `Rejected` (via `ExplicitReject`),
   then `supersede_decision(P, ..)` is `AdmissionPolicy`; repeat with
   `Contradicted`; the `Quarantined` case exists.
5. Class continuity probe (documents L2): `P` admitted at
   `(ExplicitUser, UserExplicit)`; successor admitted at `(ModelInference,
   AssistantInference)` succeeds at the kernel; record the outcome, do not
   assert it as desired behavior.

## Investigation log

### Q: Does the kernel enforce class continuity across succession?

- Sources examined: `evaluate_admission` (`admission.rs:268-274`),
  `load_prior_for_key` (`:2186-2282`), `correct_decision_inner`.
- Findings: the class check compares a request to the same subject key's
  prior; a successor is a new `AdmissionKey::Object`, so it has no prior and
  the check does not run. `correct_decision_inner` carries sensitivity, not
  classes.
- Missing evidence: none at HEAD; the plan's KTD3 wording attributes the
  property to the kernel.
- Conclusion: resolved with answer: no; recorded as lead L2 for the N1 pass.
