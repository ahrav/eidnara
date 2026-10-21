# sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority

## Discovery trigger

Plan KTD1: "`sensitivity` must be `Normal` because approval rows require it
(`admission.rs:843-856`)". Reading the code split the claim into two
mechanisms: AutoInject visibility (`surface_visibility`) and authority grant
(`approval_qualifies_predicate`). Neither lowers maturity.

## Evidence trail

- `crates/kernel/src/admission.rs:4116-4138` `surface_visibility`: `Secret`
  is `Hidden` on every surface; `(Sensitive, AutoInject | AutoSearch)` is
  `Hidden`; otherwise `Automatic` is `Visible` and `ExplicitLabeled` is
  `Labeled` only on `ExplicitSearch`.
- `:3321-3346` `ServedRow::visibility_on` and `visibility` apply
  `surface_visibility` with the served sensitivity.
- `:843-856` `approval_row_fields`: `sensitivity_class='normal'` among the
  required fields of the approval's own latest row.
- `:861-905` `approval_qualifies_predicate`: adds `o.sensitivity_class=
  'normal'`, `COALESCE(strictest history, 'secret') = 'normal'`, and live
  `normal` cited evidence.
- `:2595-2629` `validate_approval` walks the chain with that predicate; a
  failing member makes the approval invalid for citation and for
  `subject_grants_authority` (`:1104-1115`).
- `:353-360` `evaluate_admission` composes sensitivity as the max of the
  taint floor, candidate, prior, and predecessor classes; the maturity path
  (`:286-351`) never reads it.
- `:1605`, `:1631` the trigger's class folds into `candidate_sensitivity`.
- `:2140-2154` `load_subject_facts` takes the stricter of the registry class
  and `evidence_meta` for the subject, so an ingest replay that tightens
  classification is seen at the next admission.
- Tests: `admission.rs:4910` `visibility_matrix_and_sensitivity_ceiling_are_total`
  (full product); `crates/kernel/tests/kernel_admission.rs:4326`
  `a_hidden_approval_is_not_authority`; `:3422`
  `governing_decision_sensitivity_hides_an_object_the_registry_calls_normal`;
  `:2735` `dispositions_and_sensitivity_only_reduce_visibility`.

## Failure scenario

A directive whose cited artifact or trigger is `Sensitive` self-approves to
`Approved` and is read on `AutoInject` as `Visible`; classified text reaches
every turn. Or a `Sensitive` `adr_accepted` decision is accepted as the
approval for a model row, lifting that row above its ceiling on the strength
of content the policy forbids on automatic surfaces.

## Timing windows and dependencies

- A classification tightened after the admission (ingest replay on the cited
  evidence) changes the served class at the next read without a new row;
  the authority predicate reads `evidence_meta` live, so the approval stops
  granting at the same moment.
- `sensitivity_floor` (`:4236-4241`) forces `Sensitive` for `Personal` and
  `Unclassifiable` taints; neither applies to the plan's two class pairs.

## What a test must construct

1. A self-approved `adr_accepted` decision `A` whose `DecisionSpec.sensitivity`
   is `Sensitive` (or whose cited evidence is ingested `Sensitive`).
2. Assert the admission row: `effective_maturity == Approved`,
   `sensitivity_class == sensitive`.
3. `read_visible(.., AutoInject, ..)` omits `A`; `ExplicitSearch` serves it
   `Visible` (not `Labeled`, because the row is `Automatic`).
4. A model-class decision `M` admitted with `Verify` and
   `approval_object_id = A`: assert `outcome == Deny` and `effective_maturity
   == Candidate` (the approval is not valid).
5. Control: the same with `A` at `Normal`: `M` is `Verified` and `A` is
   `Visible` on `AutoInject`.
6. Tightening: ingest `A`'s cited evidence again asserted `Sensitive`, then
   re-read `AutoInject` and re-run step 4 on a fresh dependent.

## Investigation log

### Q: Does sensitivity ever lower effective maturity?

- Sources examined: `evaluate_admission` (`:264-389`), `write_admission`
  (`:1670-1848`).
- Findings: sensitivity is composed into the row (`:353-360`) and stored; no
  branch of the maturity computation reads it. Visibility and authority read
  it at serve and validation time.
- Missing evidence: none.
- Conclusion: resolved with answer: no; the guarantee is stated as visibility
  and authority effects only.
