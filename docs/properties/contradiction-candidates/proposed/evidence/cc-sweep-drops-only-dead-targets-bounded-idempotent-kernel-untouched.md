# cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched

Verified at the #1166 branch tip (base `ae02d9616`). Surface S-E.

## Discovery trigger

Candidate rows assert an overlap at one Kernel tip under one matcher, so a
row whose decision, evidence, incarnation, or matcher changed can never be
served again. Readers re-prove liveness, so the sweep is space reclaim. The
record asks that reclaim delete only rows a reader could not serve, stay
bounded per run, repeat to a fixed point, and leave the Kernel untouched.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/sweep.rs:69` `CandidateSweep::run`
  returns before any statement when the flag is false, counts one skip with
  no Kernel, reads `CANDIDATE_SWEEP_PAGE + 1` rows after the watermark, and
  only then captures the identified Kernel target, so every row on the page
  was written at or below that tip, or is skipped by the tip check in
  `judge`.
- `run` marks a row dead without a Kernel read when its matcher digest
  differs from `matcher_digest()` or its age exceeds `MAX_CANDIDATE_AGE`;
  these drops land even when the tip capture fails. `sweep.rs:196` `judge`
  then marks a row with a foreign `database_incarnation_id` dead, reads
  claim standing for the distinct decisions in ascending id order in chunks
  of 64 (a refused chunk is read again one decision at a time), and applies
  `crates/daemon/src/contradiction_candidates/eligibility.rs:7` `eligible`
  with a per-project `ScopeFilter`; then reads evidence liveness for the
  rows still kept. A failed read keeps the rows it would have judged.
- The `ScopeFilter` reads scope terms through
  `crates/kernel/src/scope.rs:1617` `scope_terms_within_budget`, which takes
  its reader through `reader_with_limit` under the sweep's budget, so a busy
  reader neither holds the run past its deadline nor blocks shutdown, which
  joins the worker. A failed scope read leaves the row out of the
  eligibility drop and counts one skip.
- `crates/kernel/src/claim_facts.rs:270` `claim_standing_at_within_budget`
  checks the claim bound inside the budget, then the target against the tip
  and the incarnation under the reader guard, and reads the served view plus
  one statement for the registry row, decision row, and own admission row of
  every id (`standing_sql`, `:394`). Each standing equals those fields of
  claim facts at the same target
  (`crates/kernel/tests/kernel_claim_standing.rs:111`, at every snapshot).
- `crates/kernel/src/evidence_liveness.rs:50`
  `evidence_liveness_at_within_budget` checks the id bound, then the
  incarnation under the reader guard, then the target against the tip, and
  answers per id from `evidence_meta.created_commit_seq` and
  `invalidated_commit_seq`, read for the whole batch by one statement that
  joins the requested ids to the primary key. It opens no write transaction.
- `crates/memory-store/src/contradiction_candidates.rs:807`
  `delete_candidates_as_read` deletes a row only while it holds the values
  the sweep read, so a rewrite after the read survives.
- `crates/daemon/src/memory_reviewer/lifecycle.rs:448` `pass_then_sweep`
  runs the sweep after a cleanup pass that was not cancelled, isolates a
  sweep panic, and the loop carries the watermark between passes.

## Failure scenario

A sweep that judged a row at a tip older than the row would delete a fresh
candidate whose evidence the old tip does not know. A sweep that abandoned
its page on any Kernel error would never reclaim the over-age rows on that
page. A keyed delete without the read values would remove a row a replay
had just rewritten. Each case has a constructing test.

## Timing windows and dependencies

The pass (#1167) writes rows concurrently. The tip check and the delete as
read close the two windows the sweep can observe; a row the pass writes
after the delete is judged by a later run.

## What a test must construct

Rows on live evidence with an eligible decision that differ in one drop
reason each; decisions in every disposition with their dispositions read
back through claim standing; a retired evidence row; a row above the tip; a
refused claim chunk beside Kernel-free drops; more than one page; a closed
Kernel; a cancelled run; one held Kernel reader beside a free one. All are in
`crates/daemon/tests/contradiction_candidates_sweep.rs`.

## Investigation log

### Q: Are stale and disputed rows kept? (originally needs human input)

- Sources examined: #1163 OQ5-4, #1166 resolved decisions,
  `crates/daemon/src/contradiction_candidates/eligibility.rs:7`.
- Findings: the predicate rejects only superseded, contradicted, rejected,
  and quarantined dispositions, a successor, a hidden explicit-search row,
  another domain, and a scope naming another project.
- Missing evidence: none.
- Conclusion: resolved with answer (kept).

### Q: Which read exposes retirement? (gap G4)

- Sources examined: `crates/kernel/src/evidence_liveness.rs:50`,
  `crates/kernel/src/envelope.rs` `evidence_citations` (inside a write
  envelope only), `crates/kernel/src/slice/read.rs`
  `linked_decisions_evidence_within_budget` (per decision).
- Findings: no batched read of evidence liveness at a snapshot existed;
  #1166 adds one. The sweep keys a descriptor on its evidence row.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Page size and scheduling

- Sources examined: `sweep.rs:24`, `lifecycle.rs:448`.
- Findings: 256 rows per run, one run per MemoryReviewer cleanup pass (30 s
  idle, 5 s after an advancing pass).
- Missing evidence: none.
- Conclusion: resolved with answer.
