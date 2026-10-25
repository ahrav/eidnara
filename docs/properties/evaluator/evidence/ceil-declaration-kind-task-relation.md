# ceil-declaration-kind-task-relation

## Discovery trigger
Parent specification #1119 C13, user story 5; Part B record `ceil-declaration-kind-task-relation`;
implemented by #1123.

## Evidence trail
- `crates/eval-core/src/ceiling.rs:20` `CeilingDeclaration`; `:50` `validate`;
  `:83` `parse_ceiling_declaration`.
- `crates/eval-core/src/report.rs:206` `SuiteBReport.ceilings`; `:366`
  `check_ceilings` runs after `check_identity`.
- `crates/eval-core/tests/report.rs::a_ceiling_declaration_names_its_kind_task_pair_and_pinned_revision`
  and `::an_invalid_or_repeated_ceiling_refuses_the_report_by_name`.

## Failure scenario
A `project_knowledge` ceiling naming two tasks would report a transfer bound
as a same-task bound.

## Timing windows and dependencies
None.

## What a test must construct
`Transfer` with equal tasks; `ProjectKnowledge` with different tasks; an
upper-case sha; a 64-hex sha; a blank task; a repeated declaration.

Legal witness: `{Transfer, "t1", "t2", <40 lower hex>}` round-trips. Smallest
falsifier: `{ProjectKnowledge, "t1", "t2", <sha>}` admitted.

## Investigation log
### Q: Catalog open question: Task ids are compared untrimmed, as the sample ledger compares them
- Sources examined: `crates/eval-core/src/ceiling.rs:50`, `:83`
- Findings: Task ids are compared untrimmed, as the sample ledger compares
  them; `t1` and `t1 ` are two tasks; `validate` tests tasks with `blank` and
  compares them with plain `==` (`crates/eval-core/src/ceiling.rs:50`)
- Missing evidence: an owner decision.
- Conclusion: needs human input
