# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: the `rp27/u1-identity-contract`
branch head on `main` at `8e0491225a7292ef077c675d44b94f94a24041d3`. Every
check below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/host-runtime/tests/dispatch.rs`, `cancel_waits_for_the_request_blocking_work` | Cancel settles nothing until held blocking work finishes; the charge is released once. | unaudited | Host test handler, not a daemon route or budget. |
| `crates/host-runtime/tests/dispatch.rs`, `a_blocking_work_panic_settles_as_one_internal_error` | A panicking closure settles as one internal error. | unaudited | No daemon failure class mapping. |
| `crates/daemon/src/transform_unit/host_tests.rs`, `request_cancel_waits_for_committed_transform_and_releases_scratch` | Transform units join on cancel and release scratch. | unaudited | Transform path, no `EvalBudget`. |
| `crates/storage/src/lib.rs`, `an_interruptible_read_stops_a_running_statement_and_a_later_read_is_untouched` | The scope interrupts a statement and the next read is untouched, including after unwind. | unaudited | Storage only; no request budget. |
| `crates/retrieval/tests/lexical_retrieval.rs`, `an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion` | The lexical lane reports an engine interrupt as budget exhaustion. | unaudited | Drives the scope from the test, not from a request. |
| `crates/kernel/tests/kernel_source_budgets.rs`, `bounded_capture_export_complete_commits_and_ack_preserve_fencing` | The acknowledgement succeeds and its checkpoint remains durable despite guard-drop cancellation. | unaudited | Kernel acknowledgement boundary, not an interrupted scan or daemon request. |

Suspiciously quiet areas: before this part no production caller used
`with_conn_interruptible`, and no daemon type owned a request budget.
