# disposition-approval-requirement-follows-the-restrictiveness-order

Catalog record: [catalog.md](../catalog.md#disposition-approval-requirement-follows-the-restrictiveness-order).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Plan P4 R4: "Active to Disputed needs no approval" and "never relaxes a
disposition". The `Operation::Disposition` doc (`commit.rs:152-154`): "a
transition toward a less restrictive disposition needs a valid approval".
Lens: security boundaries (authorization by disposition order), then
lifecycle transitions.

## Evidence trail

- `disposition_restrictiveness`: Active 0; Stale, Disputed, Superseded 1;
  Rejected 2; Contradicted, Quarantined 3
  (`crates/kernel/src/admission.rs:4244-4251`).
- `relaxes_disposition = restrictiveness(requested) <
  restrictiveness(current_disposition)` (`:337-339`);
  `requires_approval = (...) || relaxes_disposition` (`:340-345`);
  `denied = requires_approval && !input.approval_valid` (`:346`); the
  disposition is `requested` when not denied, else `current` (`:347-350`).
- Equal rank is not a relaxation: Stale to Disputed, Disputed to Stale,
  and any X to X land without approval. Route test `:4627-4639` covers
  Stale to Stale ("admitted, not denied"); no test covers Stale to
  Disputed.
- `approval_valid` comes from `validate_approval` over the cited or
  back-filled stored approval (`:1628-1645`); the route scopes the same
  approval (`commit.rs:720-723`) so an out-of-scope approval is
  `not_found` before evaluation (`kernel_routes.rs:4508-4519`).
- The route derives `denied` as `requested_disposition() != disposition`
  (`commit.rs:774-777`); `requested_disposition` reads the same
  `event_effect` table (`admission.rs:4387-4389`).
- A denial still writes one `admission_decisions` row with
  `outcome = deny` (`kernel_routes.rs:4500-4503`).
- Tests: route `:4398` (Active to each), `:4458` (Quarantined to Stale
  both ways); evaluator `:5208` (Rejected, Contradicted, Quarantined to
  Stale and Disputed), `:5407`.

## Failure scenario

A rank table edit that places Disputed below Stale would make Stale to
Disputed a relaxation, so it would require approval and block the P4
report on stale findings; one that places Contradicted at 1 would make
Contradicted to Disputed an equal-rank move, so `mark_disputed` would land
on a contradicted decision without approval. A `validate_approval`
regression that
returns true for a quarantined approval would let a plugin relax any
disposition by citing it.

## Timing windows and dependencies

The approval's authority is read at evaluation time inside the writer
transaction. In one envelope, an earlier operation that quarantines the
approval changes the later operation's answer; the preview refuses that
combination (`commit.rs:1208-1214`) while the commit judges it
(`kernel_routes.rs:5493`).

## What a test must construct

- Subjects in each of the six non-Active dispositions (Superseded via
  `supersede_decision`).
- Every ordered pair (prior disposition, event), 7 x 5, with no approval,
  an invalid approval (revoked, out of scope, wrong kind), and a valid
  approval.
- Assert `disposition`, `outcome`, `denied`, and the row count delta.
- Pure-function form over `EvaluationInputs` for the same grid.
- Coverage markers `disposition_relaxation_attempted`,
  `disposition_lateral_move_attempted`,
  `disposition_relaxation_with_valid_approval`.

## Investigation log

### Q: Is a lateral move at equal rank (Stale to Disputed) the intended contract for a consumer-filed report on a stale finding?

- Sources examined: `admission.rs:4244-4251`, `:337-346`; P4 R4 and KTD4
  ("tightening semantics unchanged"); route test `:4627-4639`.
- Findings: the code lands the move without approval and records
  `Disputed`; R4's "tightening only" does not name the equal-rank case.
  No test pins it either way.
- Missing evidence: an owner statement whether a stale finding may be
  re-labelled disputed by a plugin, and whether Disputed to Stale (the
  reverse lateral) should be allowed from a plugin.
- Conclusion: needs human input.
