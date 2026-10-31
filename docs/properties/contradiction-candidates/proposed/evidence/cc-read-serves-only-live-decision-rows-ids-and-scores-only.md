# cc-read-serves-only-live-decision-rows-ids-and-scores-only

Verified at the #1168 branch tip (base `cda655423`).

## Discovery trigger

The read is the only surface that shows candidate rows to an owner or an
agent, so it must answer only rows that a dispute could still cite and must
carry no descriptor or summary text.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/read.rs:209-221` binds the
  route through `management_binding` (the frozen flag) and `kernel_request`
  (scope first, then the strict body with `deny_unknown_fields`, `:30`).
- `crates/daemon/src/contradiction_candidates/read.rs:132` `decision_is_live`
  reads the one decision's claim standing at the identified tip
  (`crates/kernel/src/claim_facts.rs:270` `claim_standing_at_within_budget`:
  the registry row, decision row, own admission, and served standing, each
  read as claim facts read it) and applies `ScopeFilter` over
  `scope_terms_at_within_budget`, which compares the target's incarnation
  under the reader guard and runs under the same read budget, and the shared
  predicate (`crates/daemon/src/contradiction_candidates/eligibility.rs:7`
  `eligible`); a failure answers `not_live`
  (`crates/daemon/src/contradiction_candidates/read.rs:265`) before the
  candidate page is read, after the route's authority is judged again
  (`:264`).
- `crates/daemon/src/contradiction_candidates/read.rs:274`
  `candidates_for_decision_under_route_within` pages at most 64 rows in reader
  order with a keyset cursor, in the connection that reads the route's MODULE
  authority row (`crates/memory-store/src/contradiction_candidates.rs:679`;
  the public read at `:631`, `candidates_for_decision`, and the keyset
  statements with the `limit + 1` bound in `PageQuery::read`, `:872`).
- `crates/daemon/src/contradiction_candidates/read.rs:299` reads evidence
  liveness for the page's ids at the same target; `is_current` (`:192`) keeps
  a row only when its evidence is live, its incarnation and matcher digest
  equal the reader's, and its `publish_tip` is at or below the target, and
  `:311` counts the rest `read_filtered_stale`.
- `crates/daemon/src/contradiction_candidates/read.rs:102-110` `ItemOut` holds
  the eight wire keys, encoded once into the page bytes; `packages/opencode-plugin/src/shared/kernel-client/wire.ts:673`
  refuses any other key set.
- Equivalence tests: `crates/kernel/tests/kernel_claim_facts.rs:954`
  compares the standing read with claim facts field by field and refusal by
  refusal; `crates/kernel/tests/kernel_source_descriptors.rs:3275` answers
  liveness per requested position; `crates/memory-store/tests/contradiction_candidates.rs:1819`
  walks every page limit; `crates/daemon/src/candidate_pass_tests.rs:2877`
  shows a page body is the canonical `serde_json` encoding of its value;
  `crates/daemon/src/candidate_pass_tests.rs:2246` answers two pages inside the
  read budget while one Kernel reader is held;
  `crates/daemon/src/candidate_pass_tests.rs:2286` answers the conflict state
  when a restore lands between the claim-standing read and the scope read.

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
`crates/daemon/src/candidate_pass_tests.rs:1849`.

## Investigation log

### Q: Does the read judge rows at the same tip it reports?

- Sources examined: `crates/daemon/src/contradiction_candidates/read.rs:234-320`.
- Findings: the target is captured once; claim standing, evidence liveness, and
  `as_of` all use it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Can a foreign root reach the body parser?

- Sources examined: `crates/daemon/src/contradiction_candidates/read.rs:209-221`,
  `crates/daemon/src/candidate_pass_tests.rs:2159`.
- Findings: `kernel_request` answers `project_mismatch` before parsing; a
  foreign root with an unknown field answers the scope refusal.
- Missing evidence: none.
- Conclusion: resolved with answer.
