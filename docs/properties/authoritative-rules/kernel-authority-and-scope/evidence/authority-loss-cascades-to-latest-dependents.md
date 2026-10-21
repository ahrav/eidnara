# authority-loss-cascades-to-latest-dependents

## Discovery trigger

Plan KTD3 and R7: directive revocation is `revoke_approval` with
`demote_authority_dependents`; directive revision is `supersede_decision`
whose cascade "demotes dependents" (`write.rs:443-448` in the plan, verified
at `:448-452`). AE6 expects a model row that cited the directive to fall to
its ceiling.

## Evidence trail

- `crates/kernel/src/admission.rs:1351-1424` `revoke_approval` and
  `revoke_approval_inner`: non-blank reason; target must satisfy
  `APPROVAL_OBJECT_PREDICATE` else `NotFound`; `object_write::invalidate` on
  `decisions`; a `PendingChange` of kind `APPROVAL_REVOKE_KIND` with an audit
  that records `demoted` and `deferred` counts (`:1401-1410`).
- `:1447-1486` `demote_authority_dependents`: breadth-first over
  `load_approval_dependents`, a `visited` set keyed by `visit_key` (`:2484-2497`),
  deferral past `MAX_AUTHORITY_DEMOTIONS` (4,096, `:670`) or on another
  `policy_revision` (`:1468-1476`), traversal continues through deferred
  nodes.
- `:2505-2555` `load_approval_dependents`: `approval_object_id = ?1 AND
  elevated_support = 1 AND commit_seq IS NOT NULL` and latest-row predicates
  (`LATEST_SUBJECT_DECISION_PREDICATE` `:675-686`,
  `LATEST_LINEAGE_DECISION_PREDICATE` `:692-706`); over 1,024 is
  `AdmissionPolicy` (`:2551-2553`).
- `:1488-1524` `demote_one_dependent`: `apply_admission` with
  `ApprovalRevoked`, or `apply_lineage_revocation` for a lineage dependent.
- `:1617-1621` `remaining_support = min(prior.effective, automatic_ceiling)`;
  `:294-303` the evaluator requires it at or below current effective;
  `:338-339` `supported = remaining_support`; `:337` `historical =
  current.max(target)` with `target = current`, so history is unchanged.
- Other entry points: `with_authority_cascade` (`:1055-1082`) around every
  `record_admission` and `supersede_domain`; `retire_slice_object`
  (`crates/kernel/src/slice/write.rs:627-669`) and `correct_decision_inner`
  (`write.rs:362-456`) sample `subject_grants_authority` before invalidating
  and call `demote_dependents_if_authority_lost` (`admission.rs:1120-1134`)
  after, which runs the same walk when `validate_approval` now answers false.
- Tests: `crates/kernel/tests/kernel_admission.rs:469`, `:1582`, `:2614`,
  `:1503`, `:2030`, `:2081`, `:1087` (cap keeps revocation possible), `:1473`
  (callers cannot emit `ApprovalRevoked`).

## Failure scenario

The user revokes directive `D1`; model row `M` admitted at `Verified` with
`approval_object_id = D1` keeps `effective_maturity = Verified` and stays
`Visible` on `AutoInject`. Or the user revises `D1` to `D2`; `D1` is
invalidated, `M` still cites `D1`, and `M` is served on the strength of a
retired approval. Or a transitive dependent `N` (approved by `B`, which `D1`
approved) stays elevated because the walk stopped at `B`.

## Timing windows and dependencies

- The walk is inside the invalidating transaction; the single writer is held
  for its duration (comment `admission.rs:1440-1446`).
- A dependent past the 4,096 cap or on another policy revision keeps its
  stored elevated row; `validate_approval` already denies it as authority,
  but `decided_row` serves the stored visibility until a later write. No
  bound on that window exists.
- `elevated_support` is stored at write time (`:1715-1716`); a dependent
  resting at or below its ceiling is not a dependent for this purpose.

## What a test must construct

1. `seed_approval` for `A`; dependents `M1` (`Verify` with `approval = A`,
   model classes) and `B` (an `adr_accepted` decision admitted `Approve`
   with `approval = A` at explicit-user classes); `N` with `approval = B`.
2. Record the pre-state: `load_approval_dependents(A)` ids and each
   dependent's `(maturity, effective_maturity)`.
3. Run one of: `revoke_approval(A, reason)`, `retire_decision(A)`,
   `supersede_decision(A, spec)`, `record_admission(Quarantine on A)`.
4. For every dependent collected in step 2, and for `N`: the latest row has
   `event = approval_revoked`, `effective_maturity == min(previous effective,
   ceiling)`, `maturity` unchanged, written at the same `commit_seq`.
5. For `revoke_approval`: the audit JSON on the revoke change has `demoted ==
   count(step 2 closure)` and `deferred == 0`.
6. `read_visible(.., AutoInject)` after the commit omits `M1` and `N`.
7. Deferred branch (optional, expensive): 4,097 dependents, assert
   `deferred == 1` and the deferred row's stored visibility is unchanged.

## Investigation log

### Q: Is the deferred set's visibility window bounded?

- Sources examined: `demote_authority_dependents` doc comment
  (`admission.rs:1424-1446`), `decided_row` (`:2957-3036`), `validate_approval`.
- Findings: the comment states "chunking the visibility repair across
  transactions is the remaining work"; no code schedules it.
- Missing evidence: a design decision on whether the N1 pass needs a repair
  pass or accepts the window.
- Conclusion: needs human input.
