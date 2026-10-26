# disposition-never-raises-maturity-and-withdraws-effective-support

Catalog record: [catalog.md](../catalog.md#disposition-never-raises-maturity-and-withdraws-effective-support).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Plan P4 R4 ("the report never raises maturity"), the Problem Frame
("forces maturity to `Candidate`"), and index D3 ("authority never derives
from model confidence"). Lens: claimed safety guarantees, then data
integrity. The `DispositionEvent` doc (`commit.rs:237-238`) makes the same
claim: "never a maturity promotion".

## Evidence trail

- `event_effect` returns `(current, Some(<disposition>), <outcome>)` for
  all five events (`crates/kernel/src/admission.rs:4160-4163`), so
  `requested == current`.
- `event_can_raise_support` lists no disposition event (`:4189-4199`), so
  `raises_support` is false (`:318-319`).
- `target = if denied { current } else { requested }` and `historical =
  current.max(target)` (`:336-337`): `historical == current` for every
  disposition event.
- `supported` starts as `carried_effective` (`:338-344`), then "A
  non-active disposition withdraws support; history stays monotonic"
  forces `Maturity::Candidate` when `disposition != Active`
  (`:345-350`); `effective = historical.min(supported)` (`:351`).
- `write_admission` stores `historical_maturity` as `maturity` and
  `effective_maturity.get()` as `effective_maturity` (`:1770-1771`).
- A denied relaxation keeps `current_disposition` (`:331-334`); if that
  disposition is non-Active the forcing still applies, so a denied
  `mark_stale` on a Quarantined `Approved` subject writes
  `effective_maturity = candidate`.
- Unit tests: `non_active_dispositions_withdraw_effective_support`
  (`:5732`) uses `Enforce` on a Rejected prior, not a disposition event;
  `an_event_that_names_a_disposition_keeps_its_own_outcome` (`:5313`)
  checks outcome, not maturity. Integration:
  `dispositions_and_sensitivity_only_reduce_visibility`
  (`crates/kernel/tests/kernel_admission.rs:2735`) reads visibility.

## Failure scenario

An `EventKind` added to `event_can_raise_support`, or a route-reachable
event with a non-`current` first tuple member, would let a plugin raise
`historical` or `effective` maturity through `kernel.commit`. Separately,
removing the `:345-350` forcing would keep a disputed `Verified` decision
at `VisibilityRow::Automatic` (`visibility_row`, `:4248-4260`) on the
`auto_inject` surface.

## Timing windows and dependencies

None in time. Depends on the prior read being the object's own row
(`load_prior_decision`, `:2181`). Within one envelope, a later disposition
sees the earlier one's row as prior (`admission_latest`, `:1835-1843`).

## What a test must construct

- Subjects at `Verified` (CodeObserved trigger, `kernel_routes.rs:5136-
  5146`), `Approved` (Verify with a valid approval, `:4747-4757`), and
  `Enforced`; plus `Candidate`.
- Each of the five events, admitted and denied.
- Read `maturity` and `effective_maturity` of the written row; compare
  with the prior row.
- Pure-function form: `evaluate_admission` over every `EventKind` in the
  five, every `Maturity` for `historical`/`effective` of the prior with
  `effective <= historical`, every `Disposition`, both `approval_valid`
  values. Assert `historical_maturity == prior.historical_maturity`,
  `effective_maturity <= prior.effective_maturity`, and `disposition !=
  Active implies effective_maturity == Candidate`.
- Coverage marker `disposition_subject_above_candidate`.

## Investigation log

### Q: Is the preserved `maturity` history intended to be visible to P3/P4 readers?

- Sources examined: `admission.rs:336-351`, `:1770-1771`; P4 Problem
  Frame; P3 plan R3 ("whether a `MarkDisputed` or `Contradict` event
  landed after the served commit sequence").
- Findings: the row keeps `maturity` (historical) at its prior value and
  lowers `effective_maturity`. The plan's "forces maturity to Candidate"
  names one column; the row has two. P3's join reads event kind and
  `commit_seq`, not maturity.
- Missing evidence: an owner statement of which column a P3/P4 reader
  should present as the finding's standing after a dispute.
- Conclusion: needs human input.
