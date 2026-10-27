# cc-read-serves-only-live-decision-rows-ids-and-scores-only

Verified at the #1168 branch tip (base `cda655423`).

## Discovery trigger

The read is the only surface that shows candidate rows to an owner or an
agent, so it must answer only rows that a dispute could still cite and must
carry no descriptor or summary text.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/read.rs:116-128` binds the
  route through `management_binding` (the frozen flag) and `kernel_request`
  (scope first, then the strict body with `deny_unknown_fields`, `:29`).
- `crates/daemon/src/contradiction_candidates/read.rs:65` `decision_is_live`
  reads the one decision's claim facts at the identified tip and applies
  `ScopeFilter` and the shared `eligible` predicate; a failure answers
  `not_live` (`:162`) before the store is read.
- `crates/daemon/src/contradiction_candidates/read.rs:165`
  `candidates_for_decision` pages at most 64 rows in reader order with a
  keyset cursor (`crates/memory-store/src/contradiction_candidates.rs:335`).
- `crates/daemon/src/contradiction_candidates/read.rs:181` reads evidence
  liveness for the page's ids at the same target; `is_current` (`:99`) keeps
  a row only when its evidence is live, its incarnation and matcher digest
  equal the reader's, and its `publish_tip` is at or below the target, and
  `:191` counts the rest `read_filtered_stale`.
- `crates/daemon/src/contradiction_candidates/read.rs:46` `item` emits the
  eight wire keys; `packages/opencode-plugin/src/shared/kernel-client/wire.ts:652`
  refuses any other key set.

## Failure scenario

A row for a superseded decision or retired evidence invites a dispute the
Kernel refuses; a row from another incarnation names ids of a different
Kernel; a text field would disclose harness capture to the model.

## Timing windows and dependencies

The decision can be superseded between the pass and the read, and evidence
can retire between them; both are judged at the reader's identified target.
A pass can write a row after the target is captured; its `publish_tip` is
past `as_of` and it is filtered.

## What a test must construct

A MODULE-authority root with the flag on; 70 live rows and one row each with
retired evidence, a foreign incarnation, a stale matcher digest, and a
future tip; a superseded decision with rows; a second root. All in
`crates/daemon/src/candidate_pass_tests.rs:1242`.

## Investigation log

### Q: Does the read judge rows at the same tip it reports?

- Sources examined: `crates/daemon/src/contradiction_candidates/read.rs:150-197`.
- Findings: the target is captured once; claim facts, evidence liveness, and
  `as_of` all use it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Can a foreign root reach the body parser?

- Sources examined: `crates/daemon/src/contradiction_candidates/read.rs:116-128`,
  `crates/daemon/src/candidate_pass_tests.rs:1514`.
- Findings: `kernel_request` answers `project_mismatch` before parsing; a
  foreign root with an unknown field answers the scope refusal.
- Missing evidence: none.
- Conclusion: resolved with answer.
