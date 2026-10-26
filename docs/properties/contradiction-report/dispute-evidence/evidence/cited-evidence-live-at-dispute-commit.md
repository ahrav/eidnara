# cited-evidence-live-at-dispute-commit

## Discovery trigger

Plan P4 rule A2 and the stop rule "if the evidence cannot be validated as
live". At `3a3b916b7` the admission write checked only that the id exists
(record `admission-evidence-citation-is-existence-checked-only` in
[`../../disposition-path/catalog.md`](../../disposition-path/catalog.md)).

## Evidence trail

- `disposition_request` calls `require_visible_evidence` when the
  operation names an id (`crates/daemon/src/kernel_routes/commit.rs:719-721`).
- `require_visible_evidence` reads `Envelope::evidence_citations`, keeps
  the answer only when `citations.live`, and maps absence or a retired row
  to `KernelError::NotFound` (`commit.rs:747-750`).
- `evidence_citations` reads `invalidated_commit_seq IS NULL` for the row
  inside the envelope's transaction and returns `None` for an unknown id
  (`crates/kernel/src/envelope.rs:536-551`). The doc comment states the
  check and a citing write observe one state (`:532-535`).
- The commit path runs `disposition_request` inside `commit_before`'s
  closure (`commit.rs:1047-1062`, through `apply` at `:1000-1006` and
  `record_disposition` at `:785-791`); the preview path runs it inside
  `store.preview` (`commit.rs:877-897`, through `PreviewRun::disposition`
  at `:835`).
- The refusal maps to `invalid/not_found` and writes nothing: the commit
  closure returns `Err`, which rolls back the envelope.
- The module header states the rule and that a citation holds no
  retention (`commit.rs:17-22`, `:59-60`).

## Failure scenario

A client reads evidence E as live, the retention sweep retires E, and the
client files a dispute citing E. If the liveness read ran outside the
writer transaction, the dispute would land citing a retired row. With the
read inside the transaction, the dispute either sees the retirement
(`not_found`) or commits before it.

## Timing windows and dependencies

- The store admits one writer. A retirement and a dispute are ordered by
  the writer lock; the read and the admission write share the dispute's
  transaction. This is read from code.
- Not constructed: a retirement waiting on the writer lock while the
  dispute holds it. For the cited evidence row itself the state cannot
  be built: a dispute's evidence has a live citer in the bound project,
  and `retire_evidence` refuses evidence a live row cites with
  `Conflict` (`crates/kernel/src/slice/write.rs:585-608`). The reachable
  form is a retirement of that citer queued behind the dispute. A test
  would need a second writer thread blocked at the lock and a
  deterministic release point; no test in `crates/daemon/tests` or
  `crates/kernel/tests` builds that schedule. The record claims the
  as-of predicate only from the ordering argument.
- Constructed in sequence: a preview answers `available`, the only
  project citer retires in a separate commit, and the commit answers
  `not_found` with nothing written
  (`a_dispute_previewed_while_its_evidence_was_visible_is_judged_again_at_commit`,
  `crates/daemon/tests/kernel_routes.rs:6375`). The evidence row stays
  live; the refusal comes from the scope loop that shares the read, so
  the test shows the read runs again inside the commit transaction.
- A later retirement is permitted: the check is stated as of the row's
  `commit_seq`.

## What a test must construct

- A retired evidence row cited by a dispute: `not_found` from preview and
  commit, counts unchanged. Constructed by
  `a_dispute_citing_unusable_evidence_is_not_found_and_writes_nothing`
  (`crates/daemon/tests/kernel_routes.rs:5801`), which retires through
  `retire_evidence`.
- A never-ingested id: same answer. Constructed by the same test.
- A live, cited id that lands with the id on the row. Constructed by
  `a_dispute_citing_live_project_evidence_lands_with_the_id_and_the_reporter`
  (`kernel_routes.rs:5723`).
- The Kernel read's `live` bit for a live and a retired row and `None`
  for an unknown id. Constructed by
  `evidence_citations_name_the_scopes_of_live_citers_and_the_rows_liveness`
  (`crates/kernel/tests/kernel_slice.rs:2159`).

## Investigation log

### Q: Can a retired row still have live citers, so that liveness alone refuses it?

- Sources examined: `kernel_slice.rs:2216` ("Evidence retires only once
  nothing live cites it"), `retire_evidence`
  (`crates/kernel/src/slice/write.rs:585-609`), the capture expiry update
  (`crates/kernel/src/cas/deletion.rs:389`), the route test's retired case.
- Findings: `retire_evidence` answers `Conflict` for a row that
  `EVIDENCE_CITED_SQL` finds cited (`write.rs:592-608`), and the tests
  retire uncited rows. Whether the capture expiry path can retire a row
  that live rows still cite was not traced.
- Missing evidence: a read of the predicate guarding
  `cas/deletion.rs:389`.
- Conclusion: unresolved, needs that read. The route refuses a retired
  row before the scope loop either way.

### Q: Does the Kernel admission write check liveness for other writers?

- Sources examined: `write_admission` (`crates/kernel/src/admission.rs:1675`),
  `commit.rs:55-58`.
- Findings: no; it checks identity and the FK only.
- Missing evidence: an owner decision.
- Conclusion: needs human input.
