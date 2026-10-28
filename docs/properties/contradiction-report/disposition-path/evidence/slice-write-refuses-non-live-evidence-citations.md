# slice-write-refuses-non-live-evidence-citations

Catalog record: [catalog.md](../catalog.md#slice-write-refuses-non-live-evidence-citations).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

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
  commit.rs:276`, `:316`) flow into the specs (`:294`, `:346`) and into
  `envelope.insert_decision` / `insert_observation` (`:943`, `:1052`).
- `NotFound` maps to `invalid/not_found` (`kernel_routes/state.rs:276`),
  whose doc reads "does not exist, is not live, or is not scoped to the
  bound project" (`:79-81`).
- The whole envelope rolls back on the error: `commit_prepared_with_writer`
  returns at `operation(&mut envelope)?` (`envelope.rs:1453`) before
  `tx.commit()` (`:1580`).
- Kernel test `a_trigger_whose_evidence_is_gone_cannot_admit`
  (`crates/kernel/tests/kernel_admission.rs:4211`) covers the trigger
  branch in `validate_trigger`, a different predicate with the same
  liveness rule.

## Failure scenario

A predicate edit that drops `AND invalidated_commit_seq IS NULL` lets an
insert cite a retired evidence row; `object_state_sql` would then show
`artifact_digest = NULL` for the decision (`envelope.rs:1908-1910` joins
live evidence only) while the row's `evidence_id` points at a dead row.
The `COUNT(*)` form in `require_live_evidence` accepts a list that
repeats a live id, since `json_each` yields one row per element and each
live element joins its own `evidence_meta` row, so the count equals the
input length; a dangling element joins no row and always leaves the count
below the length, so it is refused. Whether the caller deduplicates the
input was not read.

## Timing windows and dependencies

The `harness_capture` expiry (`local_file.rs:245`, `retire_expired_capture`
at `:462`) can retire a row between the client's read and the commit; the
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

- Sources examined: `slice/write.rs:1095-1178` (all private `fn`);
  PR A's `Envelope::evidence_cited_in` (`envelope.rs:520-598`) and
  `require_visible_evidence`
  (`crates/daemon/src/kernel_routes/commit.rs:749-764`).
- Findings: at `3a3b916b7` no public read returned an evidence row's
  liveness by id. PR A adds `evidence_cited_in`, which reads the row's
  liveness and walks the scopes of its live citers inside the envelope's
  transaction; the daemon calls it and answers `not_found` for a retired
  or unknown row. The private helpers here are unchanged.
- Missing evidence: none.
- Conclusion: resolved with answer: `Envelope::evidence_cited_in`.

### Q: Does `require_live_evidence` deduplicate its input?

- Sources examined: `write.rs:1151-1178`, `:240-275`;
  `git grep append_decision_citations` at the PR A tip;
  `crates/kernel/src/review_acceptance.rs:623-634`, `:1004`, `:1069`,
  `:1081-1093`; `crates/daemon/src/memory_reviewer/settlement.rs:1128`.
- Findings: the count compares against `evidence_ids.len()`; `json_each`
  yields one row per input element and the JOIN matches each live id, so
  a repeated live id counts once per occurrence and the call passes.
  `append_decision_citations` (`write.rs:240`, `pub(crate)`) has one
  caller, `admit` (`review_acceptance.rs:1081-1093`), which the create and
  revise settlements call (`:1004`, `:1069`) and which production reaches
  through `accept_review_proposal` from the memory reviewer settlement
  (`settlement.rs:1128`). `admit` receives `validated.support`, which is
  sorted and deduplicated (`review_acceptance.rs:628-629`). The public
  single-event `append_decision_event` (`write.rs:208`) has callers only
  in tests. Portfolio gap G4; the callers were read, not changed.
- Missing evidence: none for the inventory.
- Conclusion: resolved with answer: the only caller deduplicates, so the
  count matches the distinct ids it cites.
