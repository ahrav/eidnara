# disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit

Catalog record: [catalog.md](../catalog.md#disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Wildcard lens, hunting outside the plan's frame. Index D3 says authority
never derives from model confidence; the plan says the report "adds data,
not authority". Neither names the inverse: a model-class caller
withdrawing authority. The route accepts any live admitted decision as a
subject, and an `adr_accepted` approval is one.

## Evidence trail

- Route reachability: `disposition_request` requires only kind
  `decision`, live, in scope, with a prior admission
  (`crates/daemon/src/kernel_routes/commit.rs:706-712`). Route test
  `:5493` quarantines `approval-1` (seeded by `seed_approval`,
  `kernel_routes.rs:4362-4393`, an `adr_accepted` decision admitted under
  `ExplicitUser`/`UserExplicit`) through the route and the commit is
  `available` (`:5558-5559`).
- `record_admission_inner` wraps the write in `with_authority_cascade`
  (`crates/kernel/src/admission.rs:1041-1055`); the bearer list is
  `[subject]` when `subject_grants_authority(subject)` (`:1071-1073`);
  after the write, `demote_dependents_if_authority_lost(bearer, reason)`
  runs (`:1079-1081`).
- `subject_grants_authority` is `subject_is_accepted_decision &&
  validate_approval(subject)` (`:1109-1120`).
- `demote_dependents_if_authority_lost` returns `(0, 0)` while
  `validate_approval(subject)` still holds, else runs
  `demote_authority_dependents` (`:1125-1139`).
- `validate_approval` walks `authority_chain_cte` and requires
  `approval_qualifies_predicate` on each member, whose `approval_row_
  fields` demand `disposition='active'`, `effective_maturity IN
  ('approved','enforced')` (`:843-856`). A disposition that writes
  `disposition != active` and `effective_maturity = candidate` (see
  `disposition-never-raises-maturity-and-withdraws-effective-support`)
  fails both.
- `demote_authority_dependents` walks the frontier, writes one
  `ApprovalRevoked` per dependent via `demote_one_dependent`, and defers
  past `MAX_AUTHORITY_DEMOTIONS = 4_096` (`:665-670`, `:1452-1490`). The
  doc at `:1445-1451` says a deferred dependent "already grants nothing"
  because validity is read from the chain.
- `ApprovalRevoked` lowers `effective_maturity` to `min(prior.effective,
  automatic_ceiling)` (`remaining_support`, `:1620-1625`; evaluator
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
`commit.rs:812-823`).

## What a test must construct

- `seed_approval` in scope; a dependent decision promoted to `Verified` by
  `Verify` citing it (`kernel_routes.rs:4747-4757` shows the shape).
- `kernel.commit` `mark_disputed` on the approval under `source_kind:
  "assistant"` and under `"user"`.
- Read the dependent's latest row: `event_kind = approval_revoked`,
  `commit_seq` equal to the dispute's, `effective_maturity = candidate`.
- Read `kernel.read` on `auto_inject` before and after.
- Coverage markers `disposition_on_authority_bearer` and
  `disposition_cascade_demoted_dependents`, witnessed in one commit.

PR A constructs the first, second, and third items under `source_kind:
"assistant"` (seeded approval, a dependent verified by `Verify` citing it
through the store API, a route dispute with evidence, the dependent's
`approval_revoked` row at the dispute's `commit_seq`) in
`a_dispute_on_an_authority_bearer_copies_its_reason_onto_demoted_dependents`
(`crates/daemon/tests/kernel_routes.rs:6199`). It does not dispute under
`"user"`, does not read the dependent's `effective_maturity` after the
demotion, and does not read `kernel.read`.

## Investigation log

### Q: Is a plugin-filed dispute of an approval, and its cascade, intended behavior under "the report adds data, not authority"?

- Sources examined: P4 KTD4, R4; index D3 and D7; `admission.rs:1041-
  1139`, `:843-856`; route test `:5493`.
- Findings: the code admits it and the route test exercises the quarantine
  variant without commenting on intent. The plan's authority invariants
  speak to promotion only.
- Missing evidence: an owner statement on whether model-class callers may
  withdraw user-granted authority, and whether the cascade should be
  previewed.
- Conclusion: needs human input.

### Q: How is a deferred dependent's served visibility repaired?

- Sources examined: `admission.rs:1445-1451`, `:1452-1490`.
- Findings: validity is already withdrawn through the chain read; the
  served row is not rewritten; the comment names chunked repair as
  remaining work. No repair job was found in this pass.
- Missing evidence: a maintenance path that resumes deferred demotions.
- Conclusion: partial; the gap is documented in code and out of this
  catalog's surface.

### Q: Does the preview show the cascade a dispute on an approval causes (G2)?

- Sources examined: `PreviewRun::disposition`
  (`crates/daemon/src/kernel_routes/commit.rs:827-866`), `changes_to`
  (`:537-542`), the route module header (`commit.rs:60-63`), the command
  usage (`packages/opencode-plugin/src/shared/memory-mark-command.ts:54`)
  and its test (`memory-mark-command.test.ts:163`), and PR A's
  `a_dispute_on_an_authority_bearer_copies_its_reason_onto_demoted_dependents`
  (`crates/daemon/tests/kernel_routes.rs:6199`).
- Findings: the preview computes `current` and `projected` for the
  subject only and sets `visibility_changes` from that pair; dependents the
  commit demotes in the same `commit_seq` (asserted by the PR A route test)
  are not in the preview. OQ4-10 resolved to document the limit: the
  header says "the preview reports the subject's visibility only" and the
  usage says "The preview reports the named memory's visibility only; a
  dispute on an approval also demotes the memories that rest on it in the
  same commit."
- Missing evidence: none.
- Conclusion: resolved with answer: documented limit; the preview does
  not report the cascade.
