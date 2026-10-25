# slice-write-refuses-non-live-evidence-citations

Catalog record: [catalog.md](../catalog.md#slice-write-refuses-non-live-evidence-citations).
HEAD `3a3b916b7`, 2026-10-25.

## Discovery trigger

Plan P4 U1 Approach step 2: "Resolve `evidence_id` to a live
`evidence_meta` row ... via the Kernel slice read API." Shared-facts lists
`require_live_evidence` and `require_optional_live` as the existing
liveness primitives. Lens: data integrity. The record pins the predicate
P4 says it will reuse so its semantics are stated before anything builds
on them.

## Evidence trail

- `require_live`: `SELECT 1 FROM {table} WHERE {column}=?1 AND
  invalidated_commit_seq IS NULL`, else `NotFound`
  (`crates/kernel/src/slice/write.rs:1138-1149`).
- `require_optional_live` applies it when the value is `Some`
  (`:1110-1120`).
- `require_parents` checks `domains`, then optional `propositions`,
  `scopes`, `anchors`, `evidence_meta` (`:1095-1108`); called by
  `insert_decision` (`:920-932`) and `insert_observation` (`:1003-1015`).
- `require_live_evidence` counts live rows for a JSON array of ids and
  requires the count to equal the input length (`:1151-1178`); called by
  `append_decision_citations` (`:240`, call at `:266`).
- Route reachability: `DecisionRequest.evidence_id` and
  `ObservationRequest.evidence_id` (`crates/daemon/src/kernel_routes/
  commit.rs:146`, `:186`) flow into the specs (`:164`, `:216`) and into
  `envelope.insert_decision` / `insert_observation` (`:783`, `:896`).
- `NotFound` maps to `invalid/not_found` (`kernel_routes/state.rs:276`),
  whose doc reads "does not exist, is not live, or is not scoped to the
  bound project" (`:79-81`).
- The whole envelope rolls back on the error: `commit_prepared_with_writer`
  returns at `operation(&mut envelope)?` (`envelope.rs:1373`) before
  `tx.commit()` (`:1500`).
- Kernel test `a_trigger_whose_evidence_is_gone_cannot_admit`
  (`crates/kernel/tests/kernel_admission.rs:4211`) covers the trigger
  branch in `validate_trigger`, a different predicate with the same
  liveness rule.

## Failure scenario

A predicate edit that drops `AND invalidated_commit_seq IS NULL` lets an
insert cite a retired evidence row; `object_state_sql` would then show
`artifact_digest = NULL` for the decision (`envelope.rs:1828-1830` joins
live evidence only) while the row's `evidence_id` points at a dead row.
The duplicate `COUNT(*)` form in `require_live_evidence` also admits a
list with a repeated live id and one dangling id if the count happens to
match; the input is deduplicated by the caller or not (not read).

## Timing windows and dependencies

The `harness_capture` expiry (`local_file.rs:245`, `retire_expired_capture`
at `:447`) can retire a row between the client's read and the commit; the
predicate runs inside the writer transaction and sees the retirement.

## What a test must construct

- Ingest an evidence row, retire it, then `kernel.commit` `insert_decision`
  citing it: assert `invalid/not_found` and unchanged `commit_log`,
  `object_registry`, `decisions` counts.
- The same with a never-existing id: same answer.
- `append_decision_citations` with one live and one retired id: `NotFound`.
- Coverage marker `slice_insert_cites_retired_evidence`.

## Investigation log

### Q: Which read-side helper will the P4 daemon check call?

- Sources examined: `slice/write.rs:1095-1178` (all private `fn`),
  `envelope.rs:1819-1833` (`object_state_sql` joins live evidence by the
  citing decision, not by evidence id), P4 U1 Files ("evidence liveness
  lookup via the Kernel slice read API").
- Findings: the liveness helpers are private to the write module and run
  inside the writer transaction; no public read returns an evidence row's
  liveness by `evidence_id` was found in this pass.
- Missing evidence: a read API in `crates/kernel/src/slice/` or
  `envelope.rs` exposing evidence liveness; not searched exhaustively.
- Conclusion: needs human input.

### Q: Does `require_live_evidence` deduplicate its input?

- Sources examined: `write.rs:1151-1178`, `:262-266`.
- Findings: the count compares against `evidence_ids.len()`; `json_each`
  yields one row per input element and the JOIN matches each live id, so
  a repeated live id counts twice. Whether the caller deduplicates was not
  read.
- Missing evidence: the caller's construction of `evidence_ids`.
- Conclusion: unresolved, needs a read of `append_decision_citations`
  callers.
