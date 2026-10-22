# kd-p10-request-local-knowledge

## Discovery trigger

Proposed obligation KD-P10 of #1060's Property Catalog section B (R12).
Tickets #1062 and #1064; PRs #1069 and #1071. Surface: Pi.

## Evidence trail

All references are verified at `07ea883b5`, the head of #1071, which holds
#1068 to #1071.

- Synthetic rows take `eidnara:synthetic:` ids
  (`crates/daemon/src/codec/pi.rs:38`, `:1537`).
- Pi runs each `context` handler over a clone of the session-derived array
  and compacts branch entries, never the transformed array
  (`@earendil-works/pi-coding-agent` 0.80.2, per #1060's dependencies).
- Witnesses: `packages/pi-plugin/src/transform/pi-delivery.test.ts:184` (no
  synthetic id in any submitted row over three turns) and `:290` (none
  across one Pi compaction, whose summary row leads the next pass);
  `packages/e2e-tests/tests/pi-rust-delivery.test.ts:109` (Pi's compaction
  requests carry no `<project-memory>` block and no `<session-history>`
  shell).

## Failure scenario

A re-ingested synthetic prefix would serve every block twice and grow each
request.

## Timing windows and dependencies

Host compaction between turns.

## What a test must construct

Three turns and one Pi compaction.

## Investigation log

### Q: Can Pi's compaction summary contain m0 text?

- Sources examined: `pi-rust-delivery.test.ts:184-212`.
- Findings: The witness asserts the input side: both compaction requests
  exclude both markers (`:197-201`). The summary text itself is the model's
  output (`:204-212` check only that the stored summary reaches the next
  request), so the record's guarantee is scoped to the compaction input.
- Missing evidence: None for the input clause; the summary's content is not
  under the adapter's control and is not claimed.
- Conclusion: resolved with answer.
