# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: the `rp27/u1-identity-contract`
branch head on `main` at `8e0491225a7292ef077c675d44b94f94a24041d3` for the
budget rows; `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98` for the route rows. Every
check below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/host-runtime/tests/dispatch.rs`, `cancel_waits_for_the_request_blocking_work` | Cancel settles nothing until held blocking work finishes; the charge is released once. | unaudited | Host test handler, not a daemon route or budget. |
| `crates/host-runtime/tests/dispatch.rs`, `a_blocking_work_panic_settles_as_one_internal_error` | A panicking closure settles as one internal error. | unaudited | No daemon failure class mapping. |
| `crates/daemon/src/transform_unit/host_tests.rs`, `request_cancel_waits_for_committed_transform_and_releases_scratch` | Transform units join on cancel and release scratch. | unaudited | Transform path, no `EvalBudget`. |
| `crates/storage/src/lib.rs`, `an_interruptible_read_stops_a_running_statement_and_a_later_read_is_untouched` | The scope interrupts a statement and the next read is untouched, including after unwind. | unaudited | Storage only; no request budget. |
| `crates/retrieval/tests/lexical_retrieval.rs`, `an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion` | The lexical lane reports an engine interrupt as budget exhaustion. | unaudited | Drives the scope from the test, not from a request. |
| `crates/retrieval/tests/exact_resolve.rs`, `an_interrupt_inside_an_exact_page_statement_stops_resolution_as_budget_exhaustion` | A page statement interrupted at every poll position returns `Projection(Interrupted)` from `page` and stops resolution as `Err(BudgetExhausted)` before any completed page or `Incomplete(BudgetExhausted)` after one, with accounting for judged pages only; a later request on the same connection completes. | unaudited | Drives the scope from the test, not from a request; the certified route lane calls `resolve` inside the request's interruptible read. |
| `crates/retrieval/tests/exact_resolve.rs`, `a_deadline_reached_inside_an_exact_page_statement_interrupts_it` | A deadline crossed while the first page statement runs makes `page` return `Projection(Interrupted)` and resolution return `Err(BudgetExhausted)`. | unaudited | One crossing point. |
| `crates/retrieval/tests/exact_resolve.rs`, `an_interrupted_admission_or_final_use_read_is_budget_exhaustion` | An interrupt inside the certificate-admission checkpoint read refuses as `BudgetExhausted`, and one inside the final-use checkpoint read invalidates the proof as `BudgetExhausted`; uninterrupted controls prove and validate. | unaudited | The checkpoint read is slowed through a view so the handler polls inside it. |
| `crates/retrieval/tests/exact_resolve.rs`, `an_engine_failure_while_the_budget_ends_stays_an_engine_failure` | An ordinary SQLite error raised in the step that ends the budget stays `Projection(Sqlite)`, under plain and interruptible reads. | unaudited | Injected through a view function. |
| `crates/retrieval/tests/exact_resolve.rs`, `a_classification_change_defeats_the_attempt_and_the_proof_whether_or_not_the_tip_moves` | A held classification window disqualifies an attempt as `ClassificationUnknown` and invalidates a proof at the same tip; a reclassifying artifact replay moves the tip and defeats both. | unaudited | Kernel test hook for the window. |
| `crates/daemon/tests/query_route_exact.rs`, `a_healthy_explicit_proof_is_answered_directly_and_still_validated` | A certified whole-request `id:` proof routes `Direct`, passes only the probe, exact, claim-validation, materialization, and response phases, serves every projected occurrence with its canonical reference; a forced-hybrid execution passes fusion. | unaudited | Fixture certificate at the projection checkpoint. |
| `crates/daemon/tests/query_route_exact.rs`, `direct_and_forced_hybrid_executions_agree_on_the_target_and_provenance` | Separate direct and forced-hybrid executions serve the same decision, revision, occurrences, and source tuples. | unaudited | One decision with three representations. |
| `crates/daemon/tests/query_route_exact.rs`, `every_unproven_request_hands_its_retained_rows_to_hybrid_retrieval` | Mention, multiple-selector, no-match, and page-bound resolutions route hybrid and every retained row reaches fusion with an exact contribution. | unaudited | `id:` targets cannot be ambiguous; ambiguity is a `sha:` resolver case. |
| `crates/daemon/tests/query_route_exact.rs`, `stale_authority_and_non_ok_rows_never_answer_directly` | A moved tip disqualifies as `ProjectionLag` and a retired decision's non-OK rows form no proof; eligible retained rows are served through hybrid retrieval. | unaudited | The retirement also moves the tip. |
| `crates/daemon/tests/query_route_exact.rs`, `refused_or_withdrawn_authority_falls_back_and_a_healthy_control_answers_directly` | A missing or refused certificate, a withdrawn grant, and a final-use `CanonicalChanged` each serve the baseline occurrences without a direct route; the healthy control routes `Direct`. | unaudited | Grant and commit injected through the final-use test hook. |
| `crates/daemon/tests/query_route_exact.rs`, `the_original_budget_ends_a_direct_or_resolved_request_at_every_stage` | Cancellation or a deadline at each direct phase, at final use, and at each hybrid phase of a certified mixed request ends the request typed with no later phase reached. | unaudited | Phase hooks; SQL interruption inside `resolve` is the resolver test's case. |
| `crates/daemon/tests/query_route_handler.rs`, `a_running_daemon_answers_a_certified_explicit_identifier_directly` | Through the running daemon's pin and certificate, `id:rule` routes `Direct` and `id:rule explicit contract` routes `Hybrid(Mention)` serving every directly served occurrence. | unaudited | Route observed through the test-support route log. |
| `crates/kernel/tests/kernel_source_budgets.rs`, `bounded_capture_export_complete_commits_and_ack_preserve_fencing` | The acknowledgement succeeds and its checkpoint remains durable despite guard-drop cancellation. | unaudited | Kernel acknowledgement boundary, not an interrupted scan or daemon request. |
| `crates/daemon/tests/kernel_routes.rs`, project-mismatch assertions | A `kernel.*` request naming another project root answers `invalid`/`project_mismatch` before its body is read. | unaudited | No harness comparison. |
| `crates/daemon/tests/claim_eligibility.rs`, `retrieval_adapter_agrees_with_daemon_and_kernel_on_one_snapshot` | The adapter, route, and kernel agree on eligibility; a foreign scope sees wrong scope. | unaudited | Judges caller-listed candidates, not lane output. |
| `crates/retrieval/tests/lexical_retrieval.rs`, scan and accepted bound tests | The lexical lane stops at its scan and accepted bounds and reports the reason. | unaudited | Not driven through the route. |
| `crates/retrieval/tests/exact_lookup.rs`, cursor tests | Exact pages resume from a cursor and refuse a stale one. | unaudited | No page-count bound. |
| `crates/retrieval/tests/fusion.rs`, union bound and filter tests | `fuse` refuses a union over its bound; `filter` keeps positions. | unaudited | Pure boundary; no route. |
| `crates/daemon/tests/search_replacement/disable.rs` | A family disable retains canonical state. | unaudited | Family, not route, disable. |

Suspiciously quiet areas: before this part no production caller used
`with_conn_interruptible`, and no daemon type owned a request budget; before
U3b no route compared a harness claim, composed two lanes, or revalidated a
fused set.
