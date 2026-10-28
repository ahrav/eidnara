# cc-table-rebuildable-from-evidence-bytes-and-live-decisions

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

The rows are derived state; a reset store must be repopulable from evidence
bytes.

## Evidence trail

- `crates/daemon/src/source_capture.rs:872` `keep_published` keeps the unit text
  the batch published; the Kernel retains exactly that text (exact retention
  refuses a text it would rewrite).
- `crates/daemon/src/contradiction_candidates/pass.rs:249` `score_publication`
  is a pure function of the descriptors, the tip's candidate set, and the
  matcher digest, apart from `created_at_ms`.

## Failure scenario

Scoring a normalized copy would make a rebuild from evidence bytes disagree with
the pass.

## Timing windows and dependencies

A rebuild at a later tip may differ because the candidate set changed; the test
fixes the tip.

## What a test must construct

A handler publication, a check that the retained texts equal the published ones,
a truncate, and a rescore of the live descriptors at the same tip; in
`crates/daemon/src/candidate_pass_tests.rs`.

## Investigation log

### Q: Where is V recorded?

- Sources examined: the evidence trail above.
- Findings: the row's `matcher_digest`.
- Missing evidence: none.
- Conclusion: resolved with answer.
