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
| `crates/kernel/tests/kernel_source_budgets.rs`, test-local `CancelOnDrop` | A budget cancelled on drop stops a kernel scan. | unaudited | Test-only guard; no production owner. |
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
