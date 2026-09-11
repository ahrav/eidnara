# embedding-dispatch-scan-makes-bounded-progress

## Discovery trigger

The dispatcher originally counted all eligible jobs before scanning. That count
bounded inserts during a pass, but acquiring it required work proportional to the
whole due backlog. Restarting every pass from the beginning also let a long
WrongScope prefix starve work for the requested project.

## Evidence trail

- `crates/retrieval/src/dispatch.rs` defines `DispatchCursor` as the exclusive
  `(created_at, job_id)` keyset position.
- `eligible_job_candidates` uses `idx_embedding_jobs_open_order` and an exclusive
  tuple comparison for pages after the cursor.
- `MAX_ELIGIBILITY_CANDIDATES` limits each kernel batch to 1,024 candidates.
- `MAX_ELIGIBILITY_PAGES_PER_PASS` is a private dispatcher constant of two.
- `EmbeddingDispatcher` owns an optional cursor and the project and destination
  binding that produced it, so a binding change resets stale progress.
- `pass` uses a local scan cursor to inspect at most two pages. A separate
  persistent cursor advances only across the consecutive WrongScope prefix
  before the first actionable row.
- WrongScope advances that persistent cursor without consuming the action
  budget. Once the pass finds an eligible or terminal row, the persistent
  cursor freezes at the preceding WrongScope position while the local scan may
  continue filling the action budget.
- A short or empty page resets the persistent cursor to `None` only when the
  scanned keyspace contained no action. This provides wraparound after the
  ordered keyspace is exhausted without skipping actionable rows.
- `self.cursor = wrong_scope_cursor` occurs only after terminal writes and
  selected job driving complete successfully.
- A search deadline, kernel refusal, hydration error, or drive block returns
  without committing the local cursor.

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

## Timing windows and dependencies

The progress claim assumes a fixed finite prefix and repeated successful passes.
Concurrent inserts can add work, but they do not extend one pass past two pages.
Rows inserted before the cursor are found after tail wrap.

Actionable rows deliberately prevent tail wrap. Their state transition removes
them from the open-row keyspace; until that transition succeeds, repeated
passes revisit them from the last safe WrongScope prefix.

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
- Findings: The cursor is copied locally and assigned back only at successful
  pass completion.
- Missing evidence: Other injected store-error classes are not individually
  exercised by this property.
- Conclusion: Resolved for the bounded deadline path.

### Q: Which candidates are safe for the persistent cursor to skip?

- Sources examined: dispatcher scan loop, action-budget handling, and retained
  host-job polling.
- Findings: Only the consecutive WrongScope prefix before the first action is
  safe to skip. Eligible and terminal rows require a durable disposition, and
  admitted rows require polling, so all three must remain visible to the next
  pass.
- Missing evidence: None for eligible, admitted, and terminal action classes.
- Conclusion: Resolved with a safe-prefix cursor that freezes at the first
  action and resets when its project or destination binding changes.
