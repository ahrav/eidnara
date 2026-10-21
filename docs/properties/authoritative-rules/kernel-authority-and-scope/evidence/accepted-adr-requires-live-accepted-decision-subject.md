# accepted-adr-requires-live-accepted-decision-subject

## Discovery trigger

Plan KTD1: "The handler inserts the `adr_accepted` decision first, then
records `AcceptedAdr`; `subject_is_accepted_decision` requires that order."
Cataloged as the precondition of the self-approval record.

## Evidence trail

- `crates/kernel/src/admission.rs:1526-1551` `prepare_admission`: a subject
  request resolves through `load_subject_facts`.
- `:2126-2174` `load_subject_facts`: the query requires
  `o.invalidated_commit_seq IS NULL`; an absent or retired object yields
  `Err(KernelError::NotFound)` before any evaluation.
- `:2436-2458` `subject_is_accepted_decision`: `EXISTS` over
  `object_registry o JOIN decisions d` with `APPROVAL_OBJECT_PREDICATE`
  (`:710-714`): `object_kind='decision'`, live registry row,
  `decision_kind='adr_accepted'`, live decision row.
- `:1636-1639` the flag is passed into `EvaluationInputs`.
- `:4221-4234` `admission_ceiling`: when the flag is false, the ceiling is
  the automatic one (`Verified` for explicit-user classes).
- `:324-330` `requires_approval`: `Approved > Verified`, so true;
  `denied = !approval_valid`; with no approval cited, denied.
- `:336` `target = current` when denied; `:362-363` `outcome = Deny`.
- `only_an_accepted_decision_object_self_approves`
  (`crates/kernel/tests/kernel_admission.rs:1740`) submits `AcceptedAdr` on a
  domain object with explicit-user classes and asserts `outcome == deny`
  and `effective_maturity != Approved`.
- `admission.rs:5066` evaluator test toggles `subject_is_accepted_decision`.

## Failure scenario

A caller records `AcceptedAdr` against a look-alike subject (a `memory`
decision whose summary reads like a directive, or a domain object) and the
evaluator returns `Approved`. The subject then satisfies
`approval_row_fields` and vouches for other rows.

Ordering failure in the directive route: the handler records the admission
before the decision insert lands; the kernel answers `NotFound` and the
route must not retry with a different subject.

## Timing windows and dependencies

- Same envelope: `insert_decision` writes the registry and `decisions` rows
  inside the transaction, so `subject_is_accepted_decision` sees them.
- The decision must be live at evaluation; a decision retired earlier in the
  same envelope is `NotFound` by `load_subject_facts`.

## What a test must construct

1. Subject A: no object with the id. `record_admission(AcceptedAdr, ..)`
   returns `Err(NotFound)` and the envelope commits nothing for it.
2. Subject B: a live decision with `decision_kind = "memory"`. The returned
   decision has `outcome == Deny`, `effective_maturity == Candidate`,
   `visibility == ExplicitLabeled`.
3. Subject C: a live domain object (as in the existing test). Same assertions
   as B.
4. Subject D: a live `adr_accepted` decision. `effective_maturity ==
   Approved` (the self-approval record).
5. Order check: in one envelope, call `record_admission` for D before
   `insert_decision` for D; assert `Err(NotFound)`.

## Investigation log

### Q: Does a retired `adr_accepted` decision still self-approve?

- Sources examined: `load_subject_facts` (`:2126-2174`), `APPROVAL_OBJECT_PREDICATE`.
- Findings: the facts query requires a live registry row, so a retired
  subject is `NotFound` before the predicate is consulted.
- Missing evidence: none.
- Conclusion: resolved with answer: no; the request is refused.
