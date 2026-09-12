# embedding-dispatch-scan-makes-bounded-progress

## Discovery trigger

The dispatcher originally counted all eligible jobs before scanning. That count
bounded inserts during a pass, but acquiring it required work proportional to the
whole due backlog. Restarting every pass from the beginning also let a long
WrongScope prefix starve work for the requested project.

## Evidence trail

- `crates/retrieval/src/dispatch.rs:218-245` defines `DispatchCursor` as the
  exclusive `(created_at, job_id)` keyset position.
- `open_job_candidates` uses `idx_embedding_jobs_open_order` and an exclusive
  tuple comparison for pages after the cursor. The SQL limits open rows before
  classifying a pending row as ready or deferred, so a deferred backlog cannot
  make one page scan unbounded: `crates/retrieval/src/dispatch.rs:247-321`.
- `MAX_ELIGIBILITY_CANDIDATES` limits each kernel batch to 1,024 candidates:
  `crates/kernel/src/eligibility.rs:17`.
- `MAX_ELIGIBILITY_PAGES_PER_PASS` is a private dispatcher constant of two:
  `crates/daemon/src/embedding_dispatch.rs:50`.
- `EmbeddingDispatcher` owns one scan position containing the cursor, project
  and destination binding, and earliest deferred retry crossed by that cursor.
  A binding change or `now >= revisit_at` resets stale progress:
  `crates/daemon/src/embedding_dispatch.rs:130-141,261-275`.
- `pass` uses a local scan cursor to inspect at most two pages. A separate
  dispatcher-retained cursor advances across the consecutive deferred and
  WrongScope prefix before the first actionable row:
  `crates/daemon/src/embedding_dispatch.rs:294-408`.
- WrongScope and deferred rows advance that persistent cursor without consuming
  the action budget. Crossing a deferred row records its retry time. Once the
  pass finds an eligible or terminal row, the persistent cursor and revisit time
  freeze at the preceding safe position while the local scan may continue
  filling the action budget: `crates/daemon/src/embedding_dispatch.rs:338-396`.
- A short or empty page resets the persistent cursor to `None` only when the
  scanned keyspace contained no action. This provides wraparound after the
  ordered keyspace is exhausted without skipping actionable rows:
  `crates/daemon/src/embedding_dispatch.rs:312-323,398-404`.
- Binding and due-revisit resets update `scan_position` before lane binding and
  later fallible work. A later failure can therefore retain a reset, but that
  reset only moves scanning to the beginning and cannot skip work:
  `crates/daemon/src/embedding_dispatch.rs:261-293`.
- Ordinary cursor progress is stored only after terminal writes and selected job
  driving complete successfully: `crates/daemon/src/embedding_dispatch.rs:409-437`.
- A search deadline, kernel refusal, hydration error, or drive block returns
  without committing ordinary cursor progress.
- `project_scan_cursor_advances_across_more_than_two_wrong_scope_pages` verifies
  cross-pass progress through 2,048 foreign rows:
  `crates/daemon/tests/embedding_dispatch.rs:2822-2855`.
- `deferred_row_is_revisited_when_its_retry_becomes_due` verifies the revisit
  reset: `crates/daemon/tests/embedding_dispatch.rs:2857-2912`.
- `terminal_search_deadline_preserves_the_candidate_for_retry` verifies that a
  failed terminal write does not skip its row:
  `crates/daemon/tests/embedding_dispatch.rs:1973-2021`.

## Failure scenario

Project B owns 2,048 older due jobs. Project A owns one later due job. A caller
repeatedly runs passes for project A. If each pass starts at the beginning, both
pages contain only B rows and the A row is never selected. If a pass scans until
it finds A, the work per pass is unbounded. The persistent cursor satisfies both
requirements: two pages per pass and progress across successful passes.

A second failure occurs if the cursor commits before terminal disposition. A
contended write can leave a candidate pending while the cursor moves beyond it.
The next pass then skips the unresolved row until a full wrap.

A third failure occurs when admitted jobs consume the pass budget. If the
cursor advances across those jobs, the next pass starts after them and admits
later pending work instead of polling the in-flight jobs. Freezing at the first
action makes admitted, eligible, and terminal rows reappear until their durable
state closes.

A cursor judged under one project or destination cannot be reused under
another binding. Otherwise, the second binding can begin after rows that the
first binding classified WrongScope and skip its own actionable work.

A deferred row can become actionable before the cursor wraps. Without the
recorded revisit time, admitted or newly arriving work after the cursor can keep
the row hidden until its episode deadline. Resetting when the retry becomes due
makes that row visible without rescanning the deferred prefix on every pass.

A deferred row can belong to a foreign project because readiness is classified
before kernel scope judgment. When its retry becomes due, the reset can revisit a
prefix that the requested project will classify `WrongScope`. This is bounded
retraversal: each pass still reads at most two pages of at most 1,024 rows. The
classification order is visible at `crates/retrieval/src/dispatch.rs:270-281` and
`crates/daemon/src/embedding_dispatch.rs:325-338`.

## Timing windows and dependencies

The progress claim assumes a fixed finite prefix and repeated successful passes.
Concurrent inserts can add work, but they do not extend one pass past two pages.
Rows inserted before the cursor are found after tail wrap.

Actionable rows deliberately prevent tail wrap. Their state transition removes
them from the open-row keyspace; until that transition succeeds, repeated
passes revisit them from the last safe deferred or WrongScope prefix.

Cursor durability is process-local. Restarting the daemon starts from the
beginning, which preserves safety and bounded work but loses only scan progress.

## What a test must construct

1. Insert 2,048 due jobs whose canonical scope differs from the requested
   project.
2. Insert one later due job for the requested project.
3. Reuse one dispatcher across at least two passes.
4. Assert the first pass performs no action on the requested job.
5. Assert a later pass admits and publishes that job without changing project
   authority in the projection.
6. Hold the search write lock after binding while a terminal candidate is due.
7. Assert the pass returns `SearchDeadline`, leaves the row pending, and does
   not quarantine.
8. Release the lock and assert the same dispatcher retries and obsoletes the
   candidate exactly once.
9. Admit up to `max_jobs` while host execution is blocked, then release the
   host and assert the next pass polls those same jobs instead of admitting a
   later pending row.
10. Advance a cursor under project A, switch the same dispatcher to project B,
    and assert project B's earlier actionable row is not skipped.
11. Place a deferred project-A row before a WrongScope row and an admitted
    project-A row, then advance logical time to the retry boundary and assert the
    deferred row is admitted before the carried cursor is reused.

## Investigation log

### Q: Does a two-page cap itself prove progress?

- Sources examined: dispatcher loop, keyset SQL, and the WrongScope regression.
- Findings: No. Progress also requires cursor persistence across successful
  passes and wraparound at the ordered tail.
- Missing evidence: None for the fixed-backlog integration case.
- Conclusion: Resolved with answer: cap, persistent cursor, and wrap form one
  contract.

### Q: Can a failed disposition advance the persistent cursor?

- Sources examined: `pass` return paths and cursor assignment.
- Findings: Ordinary cursor progress is copied locally and assigned back only at
  successful pass completion. Binding and due-revisit resets happen before later
  fallible work, but they only move the cursor to the beginning and cannot skip
  an unresolved row.
- Missing evidence: Other injected store-error classes are not individually
  exercised by this property.
- Conclusion: Resolved for the bounded deadline path.

### Q: Which candidates are safe for the persistent cursor to skip?

- Sources examined: dispatcher scan loop, action-budget handling, and retained
  host-job polling.
- Findings: A consecutive WrongScope or deferred prefix before the first action
  is safe to skip. Deferred rows also require a saved revisit time. Eligible and
  terminal rows require a durable disposition, and admitted rows require
  polling, so all three must remain visible to the next pass.
- Missing evidence: None for eligible, admitted, and terminal action classes.
- Conclusion: Resolved with a safe-prefix cursor that freezes at the first
  action and resets when its binding changes or its earliest deferred retry
  becomes due.
