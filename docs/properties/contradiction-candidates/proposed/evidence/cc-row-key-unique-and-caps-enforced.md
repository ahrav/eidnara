# cc-row-key-unique-and-caps-enforced

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

Rows are keyed by descriptor evidence and decision; replays and caps must keep
one row per key and bounded counts.

## Evidence trail

- `crates/memory-store/src/contradiction_candidates.rs:493`
  `replace_candidates_for_descriptor` enforces K, upserts on the key, keeps the
  higher tip, and evicts past the project cap in one transaction.
- `crates/daemon/src/contradiction_candidates/pass.rs:263` the pass cuts at M in
  publish order, counts the cut `dropped_at_cap`, and skips the write for a
  descriptor that reaches the pass with no room, so its held rows stay.
- `crates/daemon/src/harness_sources.rs:2151-2170` `stored_descriptor`: a
  replay takes the evidence row the stored descriptor cites, and a receipt
  whose observation cites no row, or a row other than the one its detail
  names, or a receipt whose result names another unit's descriptor, is `CorruptCanonicalRow`
  (`crates/kernel/src/source_descriptor.rs:911-943`;
  `a_replay_whose_descriptor_cites_no_evidence_row_is_refused_as_corrupt`,
  `a_replay_whose_descriptor_cites_another_units_evidence_is_refused_as_corrupt`,
  `a_replay_whose_receipt_names_another_units_descriptor_is_refused_as_corrupt`,
  and `a_commit_time_replay_names_the_evidence_the_stored_descriptor_cites`,
  `crates/daemon/tests/harness_sources.rs:3909`, `:3955`, `:4087`, `:4027`).
- `crates/daemon/src/harness_sources.rs:559` `Published.replayed`: a replayed
  unit is scored again (G3); a revision's predecessor descriptor is retired in
  the publishing commit, while its evidence stays live, so the predecessor's
  rows stay until the sweep or the age bound removes them.

## Failure scenario

An empty write past M would delete a replayed descriptor's rows; an unbounded
pass would grow the table by project size per publication.

## Timing windows and dependencies

Two passes never interleave (P = 1).

## What a test must construct

K + 1 overlaps, a pass over M, a project cap, a rescore at a newer tip, a
descriptor past M with held rows, a replay through the job; in
`crates/daemon/src/candidate_pass_tests.rs` and the store tests.

## Investigation log

### Q: K, M, and replay (OQ5-3)

- Sources examined: the evidence trail above.
- Findings: K = 4, M = 64, replayed units re-scored.
- Missing evidence: none.
- Conclusion: resolved with answer.
