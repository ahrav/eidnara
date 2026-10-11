# lexical-work-is-bounded-and-observed

## Discovery trigger

#391 AC5: carry the original `EvalBudget` through every probe and canonical
validation; observe exact and over-bound probes, allocations, SQL steps,
scanned work, and canonical batches; a result cap alone does not bound work;
missing approved limits fail closed. The RP2.3 property bundle is unavailable
here, so the record is reconstructed from the ticket's criterion and checked
against the code on the #391 branch. PR #967 maps this record to AC5 under
"work observation" and "counters at and around the bound", and keeps the
allocation evidence `partial` because RP2.9 has approved no allocation bound.

## Evidence trail

Production code, `crates/retrieval/src/lexical/retrieve.rs` unless noted:

- `RetrievalBounds` (`:42`) carries `max_probes`, `scan_rows`,
  `max_accepted`, `batch_rows`, `qualifying_matches`, and `rank_budget`, all
  `NonZeroUsize`, so every bound is caller-supplied and positive.
- `Consumed` (`:107`) reports `probes`, `scanned_rows`, `counted_rows`,
  `ranked_matches`, `judged`, `batches`, `sql_steps` (`:119`), and
  `excluded`. The `sql_steps` doc says it counts `SQLITE_STMTSTATUS_VM_STEP`
  for the count, scan, and lookup statements of the probes that finished, and
  that FTS5's rank function reads document sizes through a statement FTS5
  keeps internal, so that scoring is observed only through `ranked_matches`.
- `scan` (`:349`) checks the budget first (`:355`), refuses `max_accepted` or
  `batch_rows` above `MAX_ELIGIBILITY_CANDIDATES`
  (`crates/kernel/src/eligibility.rs:21`, 1024) as `BatchOverBound`, and
  refuses more probes than `max_probes` as `ProbesOverBound` (`:366`) before
  any statement runs. Each distinct probe is counted once; `counted_rows` and
  `sql_steps` accumulate at `:395`. A probe over `qualifying_matches` is
  common (`:406`); a qualifying probe whose count would exceed `rank_budget`
  sets `RankBudget` and stops ranking (`:423`). Each run adds `scanned_rows`
  and `sql_steps` at `:446` and marks `ScanBound` when truncated.
- `count_probe` (`:566`) reads at most `qualifying_matches + 1` rows in rowid
  order; `vm_steps` (`:584`) reads the reset statement counter. `scan_probe`
  (`:594`) dispatches to `scan_whole` (`:623`), `scan_ranked` (`:649`) with
  `ranked_rows` (`:712`), or `scan_common` (`:828`), each bounded by
  `scan_rows` and each returning its VM steps.
- `admit` (`:473`) and `admit_batches` (`:958`) judge in `batch_rows` chunks
  and stop at `max_accepted` (`:969`); `judge_batch` (`:925`) increments
  `batches` and `judged` (`:947`, `:948`).
- The daemon installs the production bounds through
  `QueryRouteLimits::production` (`crates/daemon/src/query_route.rs:135`:
  16 probes, 4096 scan rows, 128 accepted, 20,000 qualifying, 30,000 rank
  budget, 128 validation batch) and `lexical_retrieval_bounds` (`:170`).

Tests, `crates/retrieval/tests/lexical_retrieval.rs`:

- `:789` `the_scan_bound_marks_incomplete_only_past_the_bound_and_refusals_precede_every_probe`:
  `parse` matches four corpus rows. `scan_rows` of one yields `ScanBound`, one
  contribution, `scanned_rows` 1; `scan_rows` of four yields `Complete` and
  `scanned_rows` 4. Three probes under `max_probes` 2 refuse with
  `ProbesOverBound { probes: 3, bound: 2 }`; `max_accepted` or `batch_rows` of
  1025 refuses with `BatchOverBound`; a cancelled budget refuses with
  `BudgetExhausted`.
- `:1069` `a_repeated_probe_runs_the_engine_once_and_adds_no_work`: 2500 bulk
  rows plus the corpus; `parse` four times reports `probes` 4 and the same
  `scanned_rows` (2504), `counted_rows`, and `ranked_matches` as one probe;
  every contribution keeps ordinal 0; the interrupt handler polls fewer than
  twice as often as for one probe.
- `:1576` `ranking_work_is_admitted_by_exact_counts_at_the_d26b_boundaries`:
  `project_thresholds` (`:1549`) builds 20,001 rows. Probes of 19,999 and
  20,000 matches count and rank exactly; 20,001 counts one lookahead row,
  ranks zero, and is `CommonTerms` with `scanned_rows` equal to `scan_rows`.
  Pairs summing to 29,999 and 30,000 rank both; 30,001 ranks only the 15,000
  probe and is `RankBudget`, in either request order.
- `:2381` `work_counters_report_exact_and_over_bound_work`: `parse parse` at
  `scan_rows` 4 reports probes 2, counted 4, scanned 4, ranked 4, `(judged,
  batches)` of `(8, 3)` under `batch_rows` 2 (`bounds()`, `:153`), and
  `sql_steps > 0`. `scan_rows` 3 and 2 keep counted 4 and scan fewer rows with
  fewer steps. `max_accepted` 1 keeps scanned rows and steps equal to the
  uncapped run. Under `small_thresholds` (`:2136`, qualifying 2, budget 2)
  one more matching row adds one scanned row and more steps; a fifth ranked
  row adds steps over four.
- `:2312` `allocations_follow_the_scan_bound_and_the_rank_budget`:
  `retrieve_recorded` (`:418`) wraps only the `retrieve` call in
  `alloc_recorder::record_window` (`crates/daemon/tests/support/alloc_recorder.rs:359`),
  included by `#[path]` (`:2`) as the binary's `#[global_allocator]` (`:6`).
  After a warm-up run, `scan_rows` 3 records fewer `allocation_events` and
  `requested_bytes` than 4. After 32 more `parse` rows, a common scan keeps
  counted rows, scanned rows, SQL steps, and `allocation_events`; a ranked run
  reports `ranked_matches` 36 with `scanned_rows` 3 and more events than the
  three-row run. Each ledger asserts `!overflow` (`Ledger`, `:165`).
- `:2542` `a_bound_that_ends_at_a_rank_change_reads_one_row_of_the_next_rank_group`:
  64 head rows and 320 tail rows; a tied tail runs the same `sql_steps` as a
  tail with one rank per row, with `scanned_rows` 64 and `ranked_matches` 384.

## Failure scenario

A request whose result cap is one still scans, ranks, or judges every match
of a common term, so a small answer hides a projection-sized scan. Under the
production route this is a 5 second deadline spent inside SQLite on one
request, repeated by every client that sends a common word. With no counter,
the overrun leaves no evidence in the response or the tests, so the D26b gate
cannot attribute its latency to counting, ranking, scanning, or judgment.

## Timing windows and dependencies

None in the interleaving sense; every bound is checked in sequence inside one
call. Dependencies: `qualifying_matches` is read with one lookahead row, so a
count never exceeds the threshold plus one; `rank_budget` is checked before a
probe is ranked; `scan_rows` ends each run; `batch_rows` and `max_accepted`
are checked before each kernel batch. The budget check in `scan` and in each
row loop ends work at the original deadline, covered by
`original-budget-stops-sql`.

## What a test must construct

- A populated fixture engine with a probe of known match count: four `parse`
  rows in the corpus, 2500 bulk rows, or the 20,001 threshold rows.
- A repeated probe (`parse parse`) to show repeats add no engine work.
- `scan_rows` one below the match count, and one row more than the count.
- A result cap (`max_accepted`) of one against the uncapped run.
- Thresholds that put the same probe on the ranked path (`bounds()`) and on
  the common path (`small_thresholds()`), then one more matching row under
  each, and 32 more rows past the common thresholds and the scan bound.
- The daemon's allocation recorder as the test binary's global allocator, a
  warm-up run before the recorded run, and a ledger with no overflow.

## Investigation log

### Q: Which allocation counter and bound does RP2.9 approve?

- Sources examined: `Ledger` fields `allocation_events`, `requested_bytes`,
  and `overflow` (`alloc_recorder.rs:168`, `:172`, `:180`); the assertions in
  `lexical_retrieval.rs:2312`; the PR #967 description; the catalog record.
- Findings: the test compares runs with each other. One row below the scan
  bound allocates fewer events and bytes; 32 more common matches add no
  events; ranked work allocates more as `ranked_matches` grows to 36. No
  absolute event or byte bound exists in the test or the route limits.
  SQLite allocates through its own allocator, outside the recorder.
- Missing evidence: an RP2.9 decision naming the counter (events, requested
  bytes, or peak live bytes) and its bound per request.
- Conclusion: needs human input.

### Q: Does the daemon report `Consumed` past the retrieval crate?

- Sources examined: `admit_lexical` (`crates/daemon/src/query_route.rs:1242`)
  and its completion match (`:1267`); a grep of `crates/daemon/src` for
  `consumed`.
- Findings: the route reads `completion`, `reasons`, and `contributions` and
  maps bound reasons to the wire lane status; nothing in the daemon reads
  `retrieval.consumed`. The counters are observed only at the retrieval
  library boundary, in the tests above.
- Missing evidence: a route or log field that carries the counters.
- Conclusion: resolved with answer - the guarantee's "reports the work it
  did" holds at `lexical::retrieve`, and the daemon drops the report.

### Q: Is `sql_steps` exercised by a projection or only a fixture engine?

- Sources examined: `Fixture::project` (`lexical_retrieval.rs:346`) and the
  tests at `:2381` and `:2312`.
- Findings: the fixture projects rows with the production `apply_batch`
  (`:376`) into a `SqliteStore` and runs `retrieve` against it; the
  daemon handler test (`crates/daemon/tests/query_route_handler.rs:247`)
  runs the lexical lane but asserts only lane status.
- Missing evidence: none for the counters themselves.
- Conclusion: resolved with answer - the counters are asserted on a projected
  store in the retrieval tests, and the daemon path asserts none of them.
