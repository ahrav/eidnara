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

- `crates/kernel/src/admission.rs:3278-3287`: signature `(surface,
  requested: i64, domain_id: &str, scope: Option<ScopeTermFilter>) ->
  Result<VisibleAsOf, KernelError>`. No `max_rows`, no `max_bytes`, no
  `EvalBudget`.
- `admission.rs:3315-3340` `visible_selected_as_of`: `served_rows(..)?
  .into_iter().filter_map(..).collect()` into `Vec<VisibleRow>`, then
  `sort_unstable_by`. Every matching row is materialized.
- `admission.rs:3587-3605` `served_rows`: `served_classes_selected(..)?
  .into_iter().map(..).collect()`; also a full `Vec`.
- `crates/kernel/src/open.rs:595-607` `lock_reader`: the plain reader lock
  used by `read_snapshot` has no deadline.
- Daemon: `read.rs:480-481`, in `read_visible_page`, applies `NewestRows`
  after the full `Vec` is returned; `visible.rows` is consumed by the `for`
  loop at `:481` with no intermediate binding, so the observable is the
  Kernel API's return value. The caller is the domain arm at `:472-477`,
  which `read_visible_within` reaches for the capture read of the `memory`
  domain. `domain_decisions_in_serving_order` (`:404-432`) calls the API for
  the canonical-memory delivery walk and returns every row that passes the
  scope filter, with no row cap.
- `kernel.read` does not call this API. A request with `object_ids` reads
  `visible_as_of_in_scope` (`read.rs:469-471`). A request with none reads
  `visible_chunk_as_of_in_scope` (`read.rs:310-319`), which ends a chunk at
  the first commit boundary that holds `min_rows` rows or passes
  `payload_budget` bytes (`admission.rs:3244-3276`).
- The daemon cap test `kernel_routes.rs:1958` constructs over-cap rows, reads
  them through `kernel.read` with no `object_ids`, and observes only the wire
  response. `crates/daemon/src/kernel_routes/read.rs:1088`
  `a_domain_selection_over_the_row_cap_keeps_the_newest_rows` puts 8,193
  same-project `memory` decisions through the domain read by way of
  `read_visible` and observes the daemon's `truncated` flag; the Kernel-side
  count of the domain read is not asserted.

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
- Inside an over-cap fixture of `memory` decisions
  (`crates/daemon/src/kernel_routes/read.rs:1088` builds one), call the Kernel API
  directly and record `rows.len()` on its return value; assert
  `sometimes(len > MAX_READ_ROWS)` across the campaign. No production marker
  is needed.
- This witness is a precondition marker: it fires on a correct system.

## Investigation log

### Q: OQ5-1. What bounded alternative exists at HEAD?

- Sources examined: `slice/read.rs:126-220` (`decision_page_within_budget`),
  `:236-243` (`decisions_for_objects_as_of_within_budget`),
  `claim_facts.rs:240-257`, `admission.rs:3220-3242`
  (`visible_as_of_in_scope` with `ids`), `admission.rs:3244-3276`
  (`visible_chunk_as_of_in_scope`).
- Findings: the page API is store-wide (no domain or scope predicate) and its
  budget bounds only the reader wait (D2). The `ids` variant of
  `visible_as_of_in_scope` narrows before SQL but needs the ids first.
  `visible_chunk_as_of_in_scope` takes a row count, a payload budget, and the
  scope prefilter, but its statement selects every object kind and domain
  (`admission.rs:3857-3858`). No bounded, domain- and scope-filtered read
  exists.
- Missing evidence: the P5 author's choice between adding a bounded variant
  and accepting a page-then-filter design.
- Conclusion: needs human input.
