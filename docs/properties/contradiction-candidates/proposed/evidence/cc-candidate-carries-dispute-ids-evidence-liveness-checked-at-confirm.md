# cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm

Verified at the #1168 branch tip (base `cda655423`).

## Discovery trigger

The confirm form copies a row's ids into P4 `dispute`, so a row must carry
the ids the dispute validates, and the confirm must stay a Kernel decision.

## Evidence trail

- `crates/daemon/src/harness_sources.rs:555` `Published.evidence_id` is the
  evidence row id a dispute cites; the pass writes it as the row's
  `evidence_id`.
- `packages/opencode-plugin/src/tools/eidnara-memory/candidates.ts:75`
  renders `confirm: {action: "dispute", event: "mark_disputed", objectId,
  evidenceId}` from the row, and `:59` refuses a page naming another memory.
- `crates/daemon/src/kernel_routes/dispute.rs` validates the evidence at
  commit; `crates/daemon/src/candidate_pass_tests.rs:2003` checks that it and
  `commit.rs` name no candidate store method.

## Failure scenario

A row carrying the `srcdesc:*` object id makes every confirm fail; a confirm
that consulted the table would let a derived row decide a Kernel write.

## Timing windows and dependencies

The harness-capture retention sweep can retire the evidence between the
listing and the confirm; the read then leaves the row out and P4 refuses the
earlier form `not_found`.

## What a test must construct

Rows written by a real pass for two decisions; a confirm from one row with
the candidate table truncated; `expire_harness_captures` past the retention;
a confirm from the other row's earlier listing. All in
`crates/daemon/src/candidate_pass_tests.rs:1897`.

## Investigation log

### Q: Does the confirm depend on the table?

- Sources examined: `crates/daemon/src/candidate_pass_tests.rs:1963`, `:1980`.
- Findings: the table is truncated before the confirm, and the confirm still
  lands `disputed`.
- Missing evidence: no call-graph check; the source check is lexical.
- Conclusion: resolved with answer.
