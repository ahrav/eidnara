# p5dep-observational-routes-read-and-take-no-background-part

HEAD `3a3b916b7`. Surface S7.

## Discovery trigger

Plan R6: "An owner observational read lists candidates per decision." P3
#1145 U4 uses the observational route family. The doc's
search-lifecycle paragraph (`host-wire-protocol.md:808`, "sends them
over a route bound with harness `cli`") and the `review.*` tests describe
routes bound with harness `cli`. Lens: security
boundaries; lifecycle.

## Evidence trail

- `crates/daemon/src/lib.rs:274-278` `SessionBinding::is_observational()`:
  `self.harness == OBSERVATIONAL_HARNESS`; doc: "authorizes and resolves its
  project like any route and is excluded from every background view".
- `lib.rs:280-281` `OBSERVATIONAL_HARNESS = "cli"`; "`harness` is scoping
  metadata the client claims on `route.open`, never authority: the value
  selects observation and grants nothing."
- `lib.rs:382-386`: when judging whether another route shares a session, an
  observational binding is excluded.
- `lib.rs:413-417` `RouteBindings::participating`: "Observational bindings
  are excluded here, before any newest-per-root selection, so an observer
  never replaces an older participating binding, an observer-only root
  contributes nothing, and closing an observer reveals no binding that was not
  already selected. This is the only view background work reads: the
  MemoryReviewer worker's projects, the scheduler's roots and schedules, and
  the search maintenance roster."
- `crates/daemon/src/kernel_routes/mod.rs:415-441`: `kernel_route_scope`
  carries `harness: binding.harness` but makes no decision on it; the project
  check and store lookup are harness-independent.
- `crates/daemon/src/memory_reviewer/wire.rs`: no reference to harness; list
  and read depend on `ReviewScope { kernel, ledger, project }` only.
- Tests: `crates/daemon/tests/memory_reviewer_wire.rs:260`
  `an_observational_route_reads_the_same_outcomes_as_an_ordinary_route`;
  `crates/daemon/src/lib.rs:35820-35858`
  `an_observational_binding_reads_its_project_and_takes_no_part_in_background_work`
  (doc at `:35506`: alone on a dormant MODULE project it enrolls the project in
  no worker, scheduler, or maintenance view; beside a live harness it neither
  replaces nor changes the views, whichever closes first).

## Failure scenario

If a P5 owner read required a participating harness, the `cli` operator
surface could not call it. If opening a `cli` route enrolled the project in
background work, an operator inspecting candidates would start the
MemoryReviewer worker or the search roster for a project that had no live
harness; the exclusion in `participating()` prevents that today and a P5
route must not add a new background view that reads `by_route` directly.

## Timing windows and dependencies

Binding order (`seq`) decides newest-per-root among participating routes;
observers are filtered before that selection.

## What a test must construct

- One participating route and one `cli` route on the same root; call the
  read family on both; assert equal answers.
- Open and close the `cli` route around a participating one; assert
  `participating()` unchanged at each step.
- A `cli` route alone on a dormant project; assert no worker/scheduler/roster
  entry.

## Investigation log

### Q: Does any background view read `by_route` without `participating()`?

- Sources examined: `lib.rs:413-417` doc claim; grep for `by_route` use.
- Findings: the doc asserts `participating()` is the only view background
  work reads. This pass did not enumerate every `by_route` reader.
- Missing evidence: a full caller audit.
- Conclusion: partial; the claim stands as documented and tested at
  `lib.rs:35511` for the three named views.
