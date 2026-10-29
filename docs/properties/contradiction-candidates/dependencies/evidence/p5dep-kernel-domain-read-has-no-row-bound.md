# p5dep-kernel-domain-read-has-no-row-bound

HEAD `3a3b916b7`; the `claim_facts.rs` reference is re-verified at the
#1166 branch tip. Surface S2.

## Discovery trigger

Plan OQ5-1: "`visible_domain_decisions_as_of_in_scope` has no budget
parameter. Decide the candidate cap." This pass confirms the absence and
records the situation the daemon bound exists for as a coverage witness, so
that tests of the bound do not pass vacuously on small fixtures. Lens:
resource boundaries, vacuity.

## Evidence trail

- `crates/kernel/src/admission.rs:3165-3176`: signature `(surface,
  requested: i64, domain_id: &str, scope: Option<ScopeTermFilter>) ->
  Result<VisibleAsOf, KernelError>`. No `max_rows`, no `max_bytes`, no
  `EvalBudget`.
- `admission.rs:3178-3231` `visible_selected_as_of`: `served_rows(..)?
  .into_iter().filter_map(..).collect()` into `Vec<VisibleRow>`, then
  `sort_unstable_by`. Every matching row is materialized.
- `admission.rs:3482-3499` `served_rows`: `served_classes_selected(..)?
  .into_iter().map(..).collect()`; also a full `Vec`.
- `crates/kernel/src/open.rs:595-607` `lock_reader`: the plain reader lock
  used by `read_snapshot` has no deadline.
- Daemon: `read.rs:191-192` applies `NewestRows` after the full `Vec` is
  returned; `visible.rows` is consumed by the `for` loop at `:192` with no
  intermediate binding, so the observable is the Kernel API's return value.
- The daemon cap test `kernel_routes.rs:1958` constructs over-cap rows but
  observes only the wire response; the Kernel-side count is not asserted.

## Failure scenario

A P5 pass on a project with N live `memory` decisions reads and sorts N rows
per published descriptor, with N unbounded by anything in the Kernel. At the
daemon's own numbers (`read.rs:26`: ~500 bytes per row before payload), 50k
decisions is ~25 MiB per pass before payloads. The plan's stop condition
"bounded work per pass regardless of project size" cannot be met with this
API alone.

Vacuity angle: a test of the daemon's `MAX_READ_ROWS` bound on a fixture with
fewer than 8193 rows passes without the heap ever evicting. The `sometimes`
witness guards against that.

## Timing windows and dependencies

None; the cost is in the read itself.

## What a test must construct

- At least 8193 visible same-project `memory` decisions at one snapshot.
- Inside the over-cap fixture (`kernel_routes.rs:1958`), call the Kernel API
  directly and record `rows.len()` on its return value; assert
  `sometimes(len > MAX_READ_ROWS)` across the campaign. No production marker
  is needed.
- This witness is a precondition marker: it fires on a correct system.

## Investigation log

### Q: OQ5-1. What bounded alternative exists at HEAD?

- Sources examined: `slice/read.rs:126-220` (`decision_page_within_budget`),
  `:236-243` (`decisions_for_objects_as_of_within_budget`),
  `claim_facts.rs:240-257`, `admission.rs:3146-3164`
  (`visible_as_of_in_scope` with `ids`).
- Findings: the page API is store-wide (no domain or scope predicate) and its
  budget bounds only the reader wait (D2). The `ids` variant of
  `visible_as_of_in_scope` narrows before SQL but needs the ids first. No
  bounded, domain- and scope-filtered read exists.
- Missing evidence: the P5 author's choice between adding a bounded variant
  and accepting a page-then-filter design.
- Conclusion: needs human input.
