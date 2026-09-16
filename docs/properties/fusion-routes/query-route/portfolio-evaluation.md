# Portfolio evaluation: query-route budget and cancellation bridge

Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `4056217efbe424256465779aa2e6cfead608f093`.
Date: 2026-09-16. Scope: the three RP2.7.U3a records in `catalog.md`, their
evidence files, `existing-checks.md`, and `fault-map.md`. No `_lenses/`
directory exists for this part; the evaluator did not take part in the
discovery and read only the on-disk artifacts and the cited source.

## Provenance and status

The evaluator ran the four lenses (harness fit, coverage balance,
implementability, wildcard) over the artifacts and verified every symbol the
catalog names against HEAD: the 19 test functions in `Exercised:` and
`Existing check:` fields all exist at the cited paths, and the four observation
points (`RequestBudget::derive`, `SearchProjection::read_under`,
`SqliteStore::with_conn_interruptible`, `RequestCtx::run_blocking`) resolve.
No tests, builds, or benchmarks ran as part of this evaluation; the author's
verification report on the change is not re-executed here. Existing checks
stay `unaudited`.

## Four-lens synthesis

| Lens | Observation | Disposition |
| --- | --- | --- |
| Harness fit | The host witnesses reuse the real-host pattern of `crates/daemon/src/transform_unit/host_tests.rs` but do not share its harness. The storage negative control gives the request-local record a falsifiable oracle. | Harness sharing is a test refactor, queued (R3). The oracle ranking is now recorded in `fault-map.md`. |
| Coverage balance | Three safety records, all `always`; no liveness record and no coverage markers. Every fault row has a constructed test. | One bounded-liveness gap queued (G1). No marker is needed while every window is constructed directly. |
| Implementability | Record 3 is `partial` because its permit and pin clauses name state that does not exist at this head. The catalog uses symbol names, not `file:line`, so rule 1 verification is by symbol. | Correctly marked; the open question already carries `(needs human input)`. No change. |
| Wildcard | `crates/daemon/src/request_budget.rs:119` clamps `remaining_ms` to the ceiling before adding it to `Instant::now()`, so an oversized value cannot overflow the deadline. The reachability evidence lived only in a preamble sentence. | No arithmetic finding. The preamble claim is a refinement (R1), applied. |

## Finding dispositions

| Finding | Classification | Verified evidence and decision | Status |
| --- | --- | --- | --- |
| R1 | refinement | The catalog preamble asserted `test-only` for every record, which `METHOD.md` rule 4 forbids because later tickets relabel records one at a time. Grep at HEAD: `RequestBudget::derive` and `read_under` have no callers outside `request_budget.rs`, its `host_tests.rs`, and `crates/daemon/tests/request_budget_reads.rs`; `read_under` is the only production caller of `with_conn_interruptible`. | Applied: the sentence is removed and each record's `Reachability:` line carries its own evidence. |
| R2 | refinement | `fault-map.md` lacked the "coverage checks to add" and "leverage ranking" contents the `METHOD.md` per-part table requires. | Applied: both sections added; coverage checks are none because every window is constructed. |
| R3 | refinement | The host witness harness duplicates the transform host-test setup rather than sharing it. Behavior-neutral. | Queued as a test refactor; not a catalog change. |
| G1 | gap | No record bounds how long a read keeps running after the stop predicate turns true. The bound exists in code: `READ_INTERRUPT_STEPS = 1_000` VM steps (`crates/storage/src/lib.rs:191`) between progress-handler polls, and `CONN_ACQUIRE_POLL = 1 ms` (`crates/storage/src/lib.rs:129`) during acquisition. The cancel tests pass with any finite delay inside their test timeout. | Queued for U3b, which owns client-visible cancellation latency. The record must state the bound in VM steps and poll intervals, not wall time. |
| B1 | bias | Every deadline is a real `Instant`; the fault map records that virtual time cannot advance it. The author reports that deadline-sensitive tests elsewhere in `daemon --lib` fail under host load while every test this change adds passes. Not re-run here. | Surfaced. A human decides whether these tests join the load-sensitive set CI is authoritative for. |
| B2 | bias | The sibling `fusion/` part, landed by RP2.7.U1 on the base branch, has the same preamble shape and no evaluation file. | Out of this part's scope; surfaced for the U1 owner. |

## Portfolio accounting

Three active records, three index rows, three evidence files. Types: three
safety, no liveness, no reachability records. Semantics: three `always`.
Reachability: three `test-only`, each with per-record evidence. Confidence:
two high, one medium. Every record is exercised (`yes`, `yes`, `partial`).
One open question, marked `(needs human input)`. Evidence files run 55, 58,
and 80 lines, inside the `METHOD.md` target.
