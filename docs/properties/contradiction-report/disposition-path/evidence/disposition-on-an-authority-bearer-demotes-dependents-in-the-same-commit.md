# disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit

Catalog record: [catalog.md](../catalog.md#disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit).
HEAD `3a3b916b7`, 2026-10-25.

## Discovery trigger

Wildcard lens, hunting outside the plan's frame. Index D3 says authority
never derives from model confidence; the plan says the report "adds data,
not authority". Neither names the inverse: a model-class caller
withdrawing authority. The route accepts any live admitted decision as a
subject, and an `adr_accepted` approval is one.

## Evidence trail

- Route reachability: `disposition_request` requires only kind
  `decision`, live, in scope, with a prior admission
  (`crates/daemon/src/kernel_routes/commit.rs:573-579`). Route test
  `:5487` quarantines `approval-1` (seeded by `seed_approval`,
  `kernel_routes.rs:4356-4387`, an `adr_accepted` decision admitted under
  `ExplicitUser`/`UserExplicit`) through the route and the commit is
  `available` (`:5552-5553`).
- `record_admission_inner` wraps the write in `with_authority_cascade`
  (`crates/kernel/src/admission.rs:1036-1050`); the bearer list is
  `[subject]` when `subject_grants_authority(subject)` (`:1066-1068`);
  after the write, `demote_dependents_if_authority_lost(bearer, reason)`
  runs (`:1074-1076`).
- `subject_grants_authority` is `subject_is_accepted_decision &&
  validate_approval(subject)` (`:1104-1115`).
- `demote_dependents_if_authority_lost` returns `(0, 0)` while
  `validate_approval(subject)` still holds, else runs
  `demote_authority_dependents` (`:1120-1134`).
- `validate_approval` walks `authority_chain_cte` and requires
  `approval_qualifies_predicate` on each member, whose `approval_row_
  fields` demand `disposition='active'`, `effective_maturity IN
  ('approved','enforced')` (`:843-856`). A disposition that writes
  `disposition != active` and `effective_maturity = candidate` (see
  `disposition-never-raises-maturity-and-withdraws-effective-support`)
  fails both.
- `demote_authority_dependents` walks the frontier, writes one
  `ApprovalRevoked` per dependent via `demote_one_dependent`, and defers
  past `MAX_AUTHORITY_DEMOTIONS = 4_096` (`:665-670`, `:1447-1485`). The
  doc at `:1440-1446` says a deferred dependent "already grants nothing"
  because validity is read from the chain.
- `ApprovalRevoked` lowers `effective_maturity` to `min(prior.effective,
  automatic_ceiling)` (`remaining_support`, `:1615-1620`; evaluator
  `:293-302`).
- Kernel test `quarantining_an_approval_demotes_its_dependents_and_ends_
  its_authority` (`kernel_admission.rs:1503`) asserts the dependent's
  latest row is `explicit_labeled` after the quarantine (`:1536-1544`).

## Failure scenario

A model-filed `mark_disputed` against a user's approval object: the
approval's own row becomes `Disputed`/`candidate`; every decision the
user promoted by citing it drops to its automatic ceiling in the same
commit; `auto_inject` stops serving them. The write is admitted because
the route copies the approval's own `ExplicitUser` classes (see
`disposition-admission-copies-the-subjects-prior-classes`), so no class
gate refuses it. Reversal needs a relaxation with a valid approval, which
a plugin cannot supply.

## Timing windows and dependencies

The cascade runs inside the invalidating transaction and holds the single
writer for its duration. A preview of the same operation shows only the
approval's own verdict; the dependents' projected visibility is not part
of the preview (`PreviewRun` reads served rows for the targets only,
`commit.rs:656-670`).

## What a test must construct

- `seed_approval` in scope; a dependent decision promoted to `Verified` by
  `Verify` citing it (`kernel_routes.rs:4741-4751` shows the shape).
- `kernel.commit` `mark_disputed` on the approval under `source_kind:
  "assistant"` and under `"user"`.
- Read the dependent's latest row: `event_kind = approval_revoked`,
  `commit_seq` equal to the dispute's, `effective_maturity = candidate`.
- Read `kernel.read` on `auto_inject` before and after.
- Coverage markers `disposition_on_authority_bearer` and
  `disposition_cascade_demoted_dependents`, witnessed in one commit.

## Investigation log

### Q: Is a plugin-filed dispute of an approval, and its cascade, intended behavior under "the report adds data, not authority"?

- Sources examined: P4 KTD4, R4; index D3 and D7; `admission.rs:1036-
  1134`, `:843-856`; route test `:5487`.
- Findings: the code admits it and the route test exercises the quarantine
  variant without commenting on intent. The plan's authority invariants
  speak to promotion only.
- Missing evidence: an owner statement on whether model-class callers may
  withdraw user-granted authority, and whether the cascade should be
  previewed.
- Conclusion: needs human input.

### Q: How is a deferred dependent's served visibility repaired?

- Sources examined: `admission.rs:1440-1446`, `:1447-1485`.
- Findings: validity is already withdrawn through the chain read; the
  served row is not rewritten; the comment names chunked repair as
  remaining work. No repair job was found in this pass.
- Missing evidence: a maintenance path that resumes deferred demotions.
- Conclusion: partial; the gap is documented in code and out of this
  catalog's surface.
