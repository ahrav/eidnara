# Portfolio evaluation: query-route budget and cancellation bridge

Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD for code references: `5c1042324516825f1dff3f72355c0e2c908b6320`;
the catalog artifacts are described as of the change that carries this file.
Date: 2026-09-16. Scope: the three RP2.7.U3a records in `catalog.md`, their
evidence files, `existing-checks.md`, and `fault-map.md`. No `_lenses/`
directory exists for this part; the evaluator did not take part in the
discovery and read only the on-disk artifacts, the cited source, and the
review threads on the pull request.

## Provenance and status

The evaluator ran the four lenses (harness fit, coverage balance,
implementability, wildcard) over the artifacts and verified every symbol the
catalog names against HEAD: the 21 test functions in `Exercised:` and
`Existing check:` fields all exist at the cited paths, and the four observation
points (`RequestBudget::derive`, `SearchProjection::read_under`,
`SqliteStore::with_conn_interruptible`, `RequestCtx::run_blocking`) resolve.
Targeted tests ran during disposition (`cargo test -p storage --lib`,
`cargo test -p daemon --lib request_budget::`, `cargo test -p daemon --test
request_budget_reads`); no builds or benchmarks beyond those. Existing checks
stay `unaudited`.

## Four-lens synthesis

| Lens | Observation | Disposition |
| --- | --- | --- |
| Harness fit | The host witnesses reuse the real-host pattern of `crates/daemon/src/transform_unit/host_tests.rs` but do not share its harness. The storage negative control gives the request-local record a falsifiable oracle. Three witnesses cancelled after a fixed sleep or spin, so a late worker could pass them without the mid-wait poll they claim. | Harness sharing queued (R3). The oracle ranking is recorded in `fault-map.md`. The sleeps are replaced by acknowledgements from inside the observed loop or callback (R6). |
| Coverage balance | Three safety records, all `always`; no liveness record and no coverage markers. Every fault row has a constructed test. The success-path handler removal named as a window had no test. | One bounded-liveness gap queued (G1). A success-path case added (R7). No marker is needed while every window is constructed directly. |
| Implementability | Record 3 is `partial` because its permit and pin clauses name state that does not exist at this head. Record 3's guarantee claimed the connection stays held through the join; `read_on` drops the `ConnGuard` when the read returns. The catalog uses symbol names, not `file:line`, so rule 1 verification is by symbol. | Partial marking stands with its `(needs human input)` question. The guarantee now separates connection ownership (ends with the read) from work-tracker ownership (ends with the join) (R4). |
| Wildcard | `RequestBudget::derive` clamps `remaining_ms` to the ceiling before adding it to `Instant::now()` (`crates/daemon/src/request_budget.rs:116`), so an oversized value cannot overflow the deadline. `SharedBudget::eval` handed out an `EvalBudget` whose flag learns of host cancellation only when `exhaustion` or the stop predicate polls. The reachability evidence lived only in a preamble sentence. | No arithmetic finding. The accessor is removed after a failing test proved the hazard (R8). The preamble claim is a refinement (R1), applied. |

## Finding dispositions

| Finding | Classification | Verified evidence and decision | Status |
| --- | --- | --- | --- |
| R1 | refinement | The catalog preamble asserted `test-only` for every record, which `METHOD.md` rule 4 forbids. Grep at HEAD: `RequestBudget::derive` and `read_under` have no callers outside `request_budget.rs`, its `host_tests.rs`, and `crates/daemon/tests/request_budget_reads.rs`; `read_under` is the only production caller of `with_conn_interruptible`. | Applied: each record's `Reachability:` line carries its own evidence. |
| R2 | refinement | `fault-map.md` lacked the "coverage checks to add" and "leverage ranking" contents the `METHOD.md` per-part table requires; `catalog.md` lacked the relationship map from the same table. | Applied: both fault-map sections and the relationship map added. |
| R3 | refinement | The host witness harness duplicates the transform host-test setup rather than sharing it. Behavior-neutral. | Queued as a test refactor; not a catalog change. |
| R4 | refinement | `read_on` (`crates/storage/src/lib.rs`) takes `ConnGuard` by value and drops it on return, before the blocking closure returns to `run_blocking`'s join. | Applied: record 3 guarantee and evidence trail corrected. |
| R5 | refinement | The kernel `CancelOnDrop` sits inside `bounded_capture_export_complete_commits_and_ack_preserve_fencing` (`crates/kernel/tests/kernel_source_budgets.rs`) and asserts acknowledgement durability, not scan interruption. | Applied in `existing-checks.md` and record 1's `Existing check:`. |
| R6 | refinement | `a_cancelled_stop_ends_the_connection_acquisition_wait_before_the_deadline`, `a_poll_that_straddles_the_guard_drop_never_reports_a_deadline`, and the two daemon cancel-during-scan witnesses cancelled after a fixed sleep or spin; `lock_conn_until` checks the deadline before its first `try_lock`, so even a short probe read could be refused by descheduling alone. | Applied: each cancels only after a poll, a completed `exhaustion()` call, or a readiness signal sent from inside the read callback. The daemon acquisition-wait test keeps its sleep: its predicate is `SharedBudget::stop_predicate`, which offers no injectable seam without a production hook. |
| R7 | refinement | Record 2 names "cancellation raised after the read returned" as a window, but only the errored-read path was tested. | Applied: `a_cancellation_after_a_successful_read_does_not_interrupt_a_later_plain_read`, whose plain read runs before any replacement handler could mask a leak. |
| R8 | refinement | RED on the unchanged code: derive, clone `shared().eval()`, cancel the token, `assert!(eval.is_exhausted())` failed. `eval` had no production caller. | Applied: accessor removed, so the `EvalBudget` cannot be held alone; the async bridge waits for the route that owns the async side (U3b). |
| G1 | gap | No record bounds how long a read keeps running after the stop predicate turns true. The bound exists in code: `READ_INTERRUPT_STEPS = 1_000` VM steps (`crates/storage/src/lib.rs:191`) between progress-handler polls, and `CONN_ACQUIRE_POLL = 1 ms` (`crates/storage/src/lib.rs:129`) during acquisition. | Queued for U3b, which owns client-visible cancellation latency. The record must state the bound in VM steps and poll intervals, not wall time. |
| G2 | gap, unverified | A SQLite busy wait inside the interruptible read runs under the standing 5 s `BUSY_TIMEOUT` (`crates/storage/src/lib.rs:127`), unshortened by the deadline or the stop predicate. In WAL mode a reader waits only during recovery or against an exclusive-locking-mode connection; an attempt to construct that against a store already holding the wal-index failed on the blocker side with `DatabaseBusy`. | Recorded as record 1's open question. No production change without a reproducible busy reader; the write path's `with_busy_timeout_until` is the candidate remedy. |
| B1 | bias | Every deadline is a real `Instant`; the fault map records that virtual time cannot advance it. The author reports deadline-sensitive tests elsewhere in `daemon --lib` fail under host load while every test this change adds passes. Not re-run under load here. | Surfaced. A human decides whether these tests join the load-sensitive set CI is authoritative for. |
| B2 | bias | The sibling `fusion/` part, landed by RP2.7.U1 on the base branch, has the same preamble shape, no evaluation file, and no relationship map. | Out of this part's scope; surfaced for the U1 owner. |
| B3 | bias | This part has no `_lenses/` directory. Its slugs were proposed by the RP2.7 specification bundle and landed by ticket, not produced by lens passes in this repository, and `METHOD.md` calls `_lenses/` working material rather than a deliverable; eight other parts also carry none. | Not created: writing lens files after the fact would fabricate discovery reasoning that did not occur (`METHOD.md` rule 2). The provenance chain is specification, ticket, record. |

## Portfolio accounting

Three active records, three index rows, three evidence files. Types: three
safety, no liveness, no reachability records. Semantics: three `always`.
Reachability: three `test-only`, each with per-record evidence. Confidence:
two high, one medium. Every record is exercised (`yes`, `yes`, `partial`).
Two open questions: one `(needs human input)` on the permit and pin census,
one unresolved on the busy-wait bound. Evidence files run 60, 62, and 82
lines, inside the `METHOD.md` target.
