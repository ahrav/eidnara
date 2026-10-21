# Lens: architecture and data flow (S1, S2)

Pass focus: components, entry points, write-to-serve paths for Kernel
admission and scope eligibility. HEAD 08de9051a.

## Write entry points that reach admission

- `Envelope::record_admission` (`crates/kernel/src/admission.rs:977`) is the
  only caller-facing admission writer. It refuses `Correct`, `Replace`, and
  `ApprovalRevoked` (`refuse_succession_events`, `:2559-2567`) and wraps the
  write in `with_authority_cascade` (`:1055-1082`).
- `Envelope::supersede_decision` (`:1312`) guards barred lineages
  (`refuse_barred_lineage`, `:1339-1349`) and delegates to
  `correct_decision` in `crates/kernel/src/slice/write.rs:354`.
- `Envelope::retire_decision` (`write.rs:543`) invalidates and runs the same
  cascade through `retire_slice_object` (`write.rs:627-669`).
- `Envelope::revoke_approval` (`admission.rs:1351`) invalidates a live
  `adr_accepted` decision and calls `demote_authority_dependents` (`:1447`).
- Daemon `kernel.commit` route (`crates/daemon/src/kernel_routes/commit.rs`)
  exposes `insert_decision`, `supersede_decision`, `retire_decision`,
  `insert_observation`, and `disposition` (`Operation`, `:84-108`). Every
  insert or supersede is admitted through `admit` with `EventKind::Other`
  (`:451-467`) under classes from `resolve_classes` (`:283-316`), which
  starts from `derive_classes` (`:240-252`). `Disposition` ops are built by
  `disposition_request` (`:566-600`) from the subject's stored classes
  (`:589-590`).

## Production writers at HEAD (verified by grep, non-test code only)

- `EventKind::AcceptedAdr`, `EventKind::ConfigObserved`,
  `EventKind::CodeObserved`, and `revoke_approval` have no production caller.
  The only daemon caller of `CodeObserved` (`crates/daemon/src/lib.rs:13884`)
  sits under `#[cfg(any(feature = "test-support", feature =
  "direct-host-fixture"))]` (`lib.rs:13517`).
- `supersede_decision` production callers: `commit.rs:842`,
  `crates/daemon/src/memory_capture.rs:1197`,
  `crates/kernel/src/review_acceptance.rs:1044` (gated by the owner's
  acceptance record).
- `retire_decision` production callers: `commit.rs:853`,
  `crates/daemon/src/claim_sources.rs:780`.

## Serve path

- `served_classes_sql` (`admission.rs:3449-3565`) joins the object's own
  latest decision and the lineage's latest source-scoped decision; the only
  scope filter in SQL is a single `(:scope_dimension, :scope_value)` pair
  (`:3548-3562`).
- `read_visible` (`crates/daemon/src/kernel_routes/read.rs:162-220`) passes
  `read_visible` passes `project.scope_term()` (Project dimension) into the SQL and then applies
  `ScopeFilter::matches` (`crates/daemon/src/kernel_routes/project.rs:194-208`)
  per row (`read.rs:178`, `:193`).
- `ScopeFilter::matches` delegates to `ProjectScope::names_project`
  (`crates/kernel/src/eligibility.rs:86-94`), which builds a
  `ScopeMatchContext` with only the Project value (`:81`) and requires
  `scope_matches == Matches` over `CanonicalScope::from_term_specs`.
- `scope_matches` (`crates/kernel/src/scope.rs:1450-1471`) is the
  conjunction of `term_matches` (`:1392-1437`); a dimension with no context
  value is `Uncertain` (`:1418-1420`).
- `read_project_memory` (`crates/daemon/src/canonical_memory.rs:141-168`)
  calls `read_visible` with `Surface::AutoInject` and
  `RowSelection::DomainDecisions("memory")`.

## Scope writers

- `ProjectBinding::scope_spec` (`project.rs:81-99`) is the only production
  scope writer found: one `exact` term on `project`. `Dimension::Entity`
  appears outside `scope.rs` only in `crates/kernel/tests/kernel_scope_algebra.rs:426`.
  `crates/kernel/tests/kernel_schema.rs:963,1246` use the string `"entity"`
  as an object kind, not a scope dimension.
- `crates/daemon/src/lib.rs:13747` builds a `ScopeTermSpec` under the
  test-support gate.

## Findings for the catalog

- The daemon filter, not the SQL, is the deciding scope judgement; the SQL is
  a superset prefilter (rows with no Project term or a redacted term pass).
- Admission classes for every plugin write are fixed by `derive_classes`;
  `Disposition` ops reuse the subject's stored classes (`commit.rs:589-590`).
