# p5dep-capture-batch-is-bounded-per-batch

HEAD `3a3b916b7`. Surface S4.

## Discovery trigger

Plan U2: "On publish settlement with the flag on, enqueue one bounded pass
for the published descriptor ids." The task names "one batch per pass bounded
to 32 units/64 KiB". The P5 pass's per-pass input size is the number of
descriptors a job publishes. Lens: resource boundaries; C9; D3.

## Evidence trail

- `crates/daemon/src/harness_sources.rs:7` module header: `publish_screened`
  refuses units larger than `MAX_SOURCE_UNIT_BYTES` whole; each batch admits
  at most `MAX_SOURCE_BATCH_UNITS` units, `MAX_SOURCE_BATCH_BYTES` of text,
  and one unit per lineage; excess units wait for a later pass.
- `harness_sources.rs:612-616`: `MAX_SOURCE_UNIT_BYTES =
  memory_store::memory_capture::MAX_CAPTURE_SOURCE_BYTES` (64 KiB,
  `crates/memory-store/src/memory_capture.rs:19`); `MAX_SOURCE_BATCH_UNITS =
  crate::memory_capture::MAX_CAPTURE_MESSAGES` (32,
  `crates/daemon/src/memory_capture.rs:36`); `MAX_SOURCE_BATCH_BYTES =
  MAX_CAPTURE_INPUT_BYTES` (64 KiB, `:37`).
- `harness_sources.rs:1515` doc of `publish_screened` (long paragraph):
  screening order, `UnitTooLarge`, `Deferred` when the next unit would exceed
  units or bytes, `Refused` for secrets and re-admission blocks, replay counts
  toward the bounds, one unit per lineage.
- `harness_sources.rs:1557` `if len > MAX_SOURCE_UNIT_BYTES { UnitTooLarge }`;
  `:1602-1606` `if admitted.len() == MAX_SOURCE_BATCH_UNITS || admitted_bytes
  + len > MAX_SOURCE_BATCH_BYTES { full = true; Deferred }`.
- `harness_sources.rs:1228-1248` `publish_batch(kernel, writes, units,
  observed_at, budget)`: prepares each unit, then `publish_prepared` commits
  all in one kernel commit; returns `Vec<Published>` in order.
- `harness_sources.rs:520-532` `Published { object_id, occurrence_id,
  evidence_object_id, replaced_object_id, replayed, admission }`.
- `source_capture.rs:832-840`: the window batch through
  `publish_screened_between(publisher, &candidates, observed_at,
  Some(&self.budget), hook)`.
- `source_capture.rs:826-830`: `if candidates.is_empty() { return
  capture_documents(..) }`; `:861`, `:885`: after the window batch (Ok or
  Err), `capture_documents` runs.
- `source_capture.rs:904-905` `capture_documents` doc: documents are captured
  "in their own batch after the window's, under the commit gate"; `:980`
  `publish_screened(publisher, &units, now, Some(&self.budget))`.
- `source_capture.rs:3` header: "publishes at most one screened batch under a
  5 s budget" (D3: the documents batch is a second `publish_screened` call).
- Tests: `source_capture_tests.rs:428` (33 units -> 32 then 1); `:2227`
  `a_refused_window_still_captures_its_documents`; `:2246`
  `a_documents_text_is_charged_with_its_window`.

## Failure scenario

If the byte or unit bound were applied per unit instead of cumulatively, a
window of many small units would publish unbounded descriptors in one commit
and the P5 pass's per-pass input would be unbounded. For D3: a P5 trigger
that counted "one batch" would under-count the descriptors a job publishes
when documents are configured (`inject_docs`), missing the document
descriptors from scoring or double-triggering if it hooked both batches.

## Timing windows and dependencies

Both batches run under the same commit gate and the same 5 s budget;
`capture_documents` checks `budget.is_exhausted()` (`:916-918`) and counts
dropped reads as `DocumentsRefused` with a `Deadline` end.

## What a test must construct

- A 33-unit window (existing); a window whose text sums past 64 KiB with
  under 32 units; a single 64 KiB + 1 unit beside a small one.
- A pass with a window and configured documents; assert two
  `publish_screened` calls (two kernel commits) in one job, each within
  bounds, via commit count or receipts.

## Investigation log

### Q: D3. Does "at most one screened batch" in the header include documents?

- Sources examined: `source_capture.rs:3`, `:826-905`, `:980`;
  `source_capture_tests.rs:2227`.
- Findings: the code publishes the window batch and a documents batch as two
  `publish_screened*` calls. The header sentence predates or omits the
  documents batch. Each batch is individually bounded.
- Missing evidence: author intent for the header.
- Conclusion: needs human input (documentation question); the per-batch
  bound is a fact.
