# cited-evidence-visible-to-bound-project

## Discovery trigger

Plan P4 rule A3 requires a cited `evidence_id` to be "visible to the
caller's project scope". At `3a3b916b7` evidence objects carried no scope
and the route's only scope mechanism answered `NotFound` for every
evidence object (record `evidence-objects-carry-no-scope-and-fail-scoped-lookup`
in [`../../disposition-path/catalog.md`](../../disposition-path/catalog.md)).
The specification recommended visibility through a scoped citer
([`../open-questions.md`](../open-questions.md) Q1); PR A implements that
definition.

## Evidence trail

- `EvidenceCitations.citing_scopes` holds the distinct non-null scope ids
  of the live decisions, observations, and asserted edges whose
  `evidence_id` is the row, and of the live decisions whose events cite
  it (`crates/kernel/src/envelope.rs:187-194`).
- The SQL is a `UNION` of four selects, each filtering
  `invalidated_commit_seq IS NULL` on the citer (the decision for the
  event branch) and `scope_id IS NOT NULL`, ordered by scope id
  (`envelope.rs:555-565`).
- `require_visible_evidence` accepts the first citing scope that
  `ScopeFilter::matches` accepts for the bound project and answers
  `NotFound` otherwise (`crates/daemon/src/kernel_routes/commit.rs:758-765`).
  `ScopeFilter` is the same algebra `kernel.read` serves rows with
  (`commit.rs:677-681`).
- Every refusal case (unknown, retired, uncited, unscoped-only,
  foreign-only) answers the one name `not_found` (doc comment at
  `commit.rs:745-748`; header at `:48-49`). Since `511cd6d73` the doc
  comment states that the check answers the same for another project's
  evidence as for an unknown id.
- The scope loop tries every citing scope until one passes
  (`commit.rs:758-764`), so the order scopes sort in does not decide the
  answer.
- `apply` runs an envelope's operations in order in one transaction
  (`commit.rs:919-1062`), so a citer inserted earlier in the envelope is
  visible to the dispute's read.
- Decision events reach the citing set through the review acceptance
  path: `append_decision_citations`
  (`crates/kernel/src/slice/write.rs:240`) is called only by `admit` in
  `crates/kernel/src/review_acceptance.rs:1088`, which runs for the create
  and revise settlements (`:1004`, `:1069`) reached from
  `accept_review_proposal` in
  `crates/daemon/src/memory_reviewer/settlement.rs:1128`. The support list
  is sorted and deduplicated first (`review_acceptance.rs:628-629`).
  Portfolio gap G4; read, not changed.

## Failure scenario

Project P's client learns evidence id E from project Q and files a
dispute in P citing E. If the route accepted any live id, the dispute
would attach Q's evidence to P's finding, and the answer (`available`
versus `not_found`) would reveal that E exists.

## Timing windows and dependencies

- The only citer of E in P is retired between the client's read and the
  commit. The read runs inside the writer transaction, as in
  `cited-evidence-live-at-dispute-commit`, so the commit sees the
  retirement.
- The scope verdict depends on stored scope terms read through
  `envelope.scope_terms` inside the same transaction (`commit.rs:759-761`).
- The sequential form (preview answers `available`, the only project
  citer retires, the commit answers `not_found` with nothing written) is
  constructed by
  `a_dispute_previewed_while_its_evidence_was_visible_is_judged_again_at_commit`
  (`crates/daemon/tests/kernel_routes.rs:6381`).

## What a test must construct

- Two projects bound in one store; evidence cited only by the other
  project's decision: `not_found`, nothing written. Constructed in
  `a_dispute_citing_unusable_evidence_is_not_found_and_writes_nothing`
  (`crates/daemon/tests/kernel_routes.rs:5801`) with `bind_project("project-b")`.
- Evidence cited only by an unscoped decision, and evidence nothing cites:
  `not_found`. Constructed in the same test.
- Evidence cited by a decision of the bound project: lands. Constructed by
  `project_evidence` in `kernel_routes.rs:5723`.
- Kernel read: an unscoped citer and a retired citer add no scope; an
  observation and a decision event (through a live decision) add theirs;
  scopes appear once each in order. Constructed by
  `evidence_citations_name_the_scopes_of_live_citers_and_the_rows_liveness`
  (`crates/kernel/tests/kernel_slice.rs:2159`).
- Evidence cited by the bound project and by a foreign scope that sorts
  first: lands; evidence cited only by the foreign scope: `not_found`.
  Constructed by
  `a_dispute_judges_every_citer_and_both_evidence_fields_together`
  (`kernel_routes.rs:6290`).
- An `insert_decision` citing uncited evidence earlier in the dispute's
  envelope: lands; the same dispute alone: `not_found`. Constructed by
  `an_insert_citing_evidence_makes_it_citable_later_in_the_same_commit`
  (`kernel_routes.rs:6481`).
- Not constructed: an asserted-edge citer. The `asserted_edges` branch of
  the SQL (`crates/kernel/src/envelope.rs:563-564`) is read, not
  exercised, and no Rust code under `crates/` writes `asserted_edges`, so
  the arm has no production writer either.

## Investigation log

### Q: Is evidence cited only by unscoped rows ever citable?

- Sources examined: the SQL filter `scope_id IS NOT NULL`
  (`envelope.rs:556-564`); the route test's unscoped case.
- Findings: no; it answers `not_found`.
- Missing evidence: an owner decision on whether that is intended.
- Conclusion: needs human input.

### Q: Which callers append decision-event citations (G4)?

- Sources examined: `git grep append_decision_citations` at the PR A tip.
- Findings: one definition (`write.rs:240`, `pub(crate)`) and one caller
  (`review_acceptance.rs:1088`); the public single-event append
  (`write.rs:208`) has callers only in tests.
- Missing evidence: none for the inventory.
- Conclusion: resolved with answer: review acceptance is the only
  production path that creates decision-event citers.

### Q: Can a client make evidence citable through the route's own inserts (portfolio G1)?

- Sources examined: `insert_decision` and `insert_observation` wire
  fields (`commit.rs:276`, `:314`); `require_parents`
  (`crates/kernel/src/slice/write.rs:1095-1107`); `apply`
  (`commit.rs:919-1062`); `kernel_routes.rs:6481`.
- Findings: yes. The inserts accept any live evidence id; the Kernel
  checks liveness only. The test refuses a dispute citing uncited live
  evidence with `not_found`, then lands one envelope holding an
  `insert_decision` citing that evidence and the dispute, with the id on
  the row. Under the resolved OQ4-3 definition, evidence is citable once
  this project cites it, in the same envelope included. The insert path
  answers `available` for a live id and `not_found` for an unknown one;
  that existence answer is pre-existing behavior outside #1160. Whether
  evidence ids are guessable was not determined.
- Missing evidence: an owner decision on whether visibility means
  "already cited by this project" or "citable by this project", and
  whether inserts get the same check.
- Conclusion: needs human input.

### Q: Is the citing-row read bounded?

- Sources examined: `Envelope::evidence_citations`
  (`envelope.rs:536-577`); [`../open-questions.md`](../open-questions.md)
  Q1, which recommends a bounded scan.
- Findings: no. The `UNION` reads every live scoped row citing the id,
  inside the writer transaction. Review disposition: the owner defers
  this; each citing row was paid for by a write, and a bounded read is a
  follow-up.
- Missing evidence: none for the behavior.
- Conclusion: resolved with answer: unbounded at `511cd6d73`; follow-up
  deferred by the owner.
