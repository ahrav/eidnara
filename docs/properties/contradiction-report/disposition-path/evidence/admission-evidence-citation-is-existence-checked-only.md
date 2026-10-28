# admission-evidence-citation-is-existence-checked-only

Catalog record: [catalog.md](../catalog.md#admission-evidence-citation-is-existence-checked-only).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Plan P4 KTD2: "The Kernel persists `evidence_id` on `admission_decisions`
(`admission.rs:1817-1852`) but the disposition path does not check
liveness." Shared-facts: "No evidence liveness check exists on this path."
Lens: data integrity, then failure recovery (what a retired citation does
to later readers). The record pins what the Kernel enforces so the P4
daemon check is specified against it.

## Evidence trail

- `write_admission` derives `evidence_id` by `identity()` only
  (`crates/kernel/src/admission.rs:1750-1755`); `identity` rejects a
  value with a detected secret (`crates/kernel/src/redaction.rs:81-88`).
- Between `admission.rs:1755` and the INSERT (`admission.rs:1816-1859`)
  the only reads are the bearer and dependent caps (`:1795-1807`); none
  touches `evidence_meta`.
- `prepare_with_facts` passes `has_evidence: request.event.evidence_id.
  is_some()` (`:1704`), a request-shape test; it matters only for
  `Enforce` (`:291-293`).
- Schema: `evidence_id TEXT REFERENCES evidence_meta(evidence_id) ON
  DELETE RESTRICT` (`crates/kernel/src/schema.rs:164`); `PRAGMA
  foreign_keys=ON` at `schema.rs:250` and `crates/kernel/src/open.rs:1035`;
  `sqlite_runtime.rs:127-129` reports a disabled pragma as a violation.
- `map_sqlite`: `ConstraintViolation => KernelError::Conflict`
  (`crates/kernel/src/lib.rs:218-230`). The route's `run` turns a
  `Conflict` raised after entering the operation, with no recorded refusal,
  into `CommitFailure::StorageConstraint`, reported as
  `invalid/already_exists` (`commit.rs:1084-1092`, `:490`).
- Production writers that set `Some(evidence_id)`:
  `crates/daemon/src/harness_sources.rs:1004` (`EventKind::Other`) only.
  `crates/daemon/src/memory_reviewer/lifecycle.rs:649` was first listed
  here; the portfolio evaluation found it inside `#[cfg(test)]` (module at
  `:500`) and an `ObservationSpec`, so it is not a writer of this path
  (refinement R1, applied 2026-10-25). At `3a3b916b7` the disposition
  route set `None`. At the PR A tip it sets the cited id for a dispute
  (`commit.rs:738`) only after `require_visible_evidence` found the row
  live and cited by the bound project in the same transaction
  (`commit.rs:726-728`, `:749-764`), so the route never reaches this
  record's accept-retired or dangling-id branch.
- Contrast: `validate_trigger` requires the trigger's backing evidence to
  be live (`admission.rs:2841-2847`) and
  `approval_qualifies_predicate` requires cited evidence live
  (`:900-909`); the event's own `evidence_id` has no such predicate.
- Served folding reads `decision_events` with
  `event_kind='memory_reviewer_support'` (`:3510-3526`), not
  `admission_decisions.evidence_id`, so a retired citation on an admission
  row changes nothing served today.

## Failure scenario

A writer cites an evidence row the retention sweep retired in the same
window. The admission row lands with a citation to a retired row. A later
reader (P3's sweep: "keeps rows a live admission event's evidence cites")
either keeps a use-outcome row forever on a dead citation or must itself
re-check liveness. A dangling id instead aborts the whole envelope with
`already_exists`, a name that describes a duplicate, not a missing
reference.

## Timing windows and dependencies

The retention sweep (`harness_capture`, `curator_capture`;
`schema.rs:136` comment) can retire a row between a caller's read and its
commit; the writer transaction serializes the write but has no predicate
to notice.

## What a test must construct

- Store API: `ingest_evidence` (`kernel_admission.rs:92`), retire the row
  (`invalidated_commit_seq` set), then `record_admission` with
  `evidence_id: Some(id)`; assert `Ok` and the row's `evidence_id`.
- Same with an id that has no row; assert `Err(Conflict)` and rollback.
- Same with an id containing a detected secret; assert
  `Err(InvalidInput)`.
- Coverage markers `admission_write_with_evidence_id`,
  `admission_write_with_retired_evidence_id`.

## Investigation log

### Q: Should the Kernel refuse a retired `evidence_id` on `write_admission`, or is the daemon the only intended gate?

- Sources examined: P4 KTD2, D5 in the index, `admission.rs:1750-1859`,
  `slice/write.rs:1138-1178`.
- Findings: the slice write path enforces liveness; the admission path
  does not. KTD2 places the check in the daemon. Nothing in the plan says
  the Kernel should also refuse.
- Missing evidence: owner decision on defense in depth.
- Conclusion: needs human input.

### Q: Which refusal name does the P4 daemon check use, given the Kernel's answer for a dangling id is `already_exists`?

- Sources examined: `state.rs:61-99` (`InvalidReason` set), `commit.rs:
  484`, P4 R2 ("refused by name").
- Findings: today's names that fit are `not_found` (the slice path's
  answer) and `already_exists` (the FK's). Neither says "evidence". PR A
  chose `not_found`: `require_visible_evidence` maps every failing case to
  `KernelError::NotFound` (`commit.rs:754-763`) before the Kernel's FK
  path could answer `already_exists`.
- Missing evidence: none.
- Conclusion: resolved with answer: `not_found`.
