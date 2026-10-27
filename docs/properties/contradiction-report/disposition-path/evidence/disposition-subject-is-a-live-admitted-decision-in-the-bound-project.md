# disposition-subject-is-a-live-admitted-decision-in-the-bound-project

Catalog record: [catalog.md](../catalog.md#disposition-subject-is-a-live-admitted-decision-in-the-bound-project).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Lens: security boundaries and lifecycle transitions. The route's
`not_found` doc (`state.rs:79-81`) names three refusal causes in one word;
the record lists the exact predicates a disposition subject must satisfy
and what each failure answers.

## Evidence trail

- `disposition_request` (`crates/daemon/src/kernel_routes/commit.rs:694-
  736`):
  1. `scoped_object_state(envelope, filter, object_id)?` (`:706`): the
     registry row exists and its `scope_id` names the bound project, else
     `NotFound`.
  2. `object_kind != "decision" || invalidated_commit_seq.is_some()` is
     `NotFound` (`:707-709`). `load_object_state` has no liveness filter
     of its own (`envelope.rs:1925-1939`), so this line is the liveness
     check.
  3. `subject_admission(object_id)?.ok_or(AdmissionPolicy)` (`:710-712`):
     a decision with no admission row is `AdmissionPolicy`.
  4. `approval_object_id.or(stored_approval)`: the approval and every
     `approval_chain_members` entry pass `scoped_object_state`
     (`:713-717`); the chain reader refuses above
     `MAX_AUTHORITY_CHAIN_DEPTH` with `AdmissionPolicy`
     (`admission.rs:1017-1038`).
- Mapping: `NotFound -> invalid/not_found`, `AdmissionPolicy ->
  invalid/admission_policy` (`kernel_routes/state.rs:275-276`).
- Both preview and commit call the same function (`commit.rs:788` in
  `record_disposition`, `:833` in `PreviewRun::disposition`), so a refusal is identical in both modes
  (`kernel_routes.rs:5355`).
- Tests: retired subject and unadmitted subject (`kernel_routes.rs:4888-
  4941`); missing id (`:5078-5086`); foreign approval chains of three
  shapes (`:4568-4577`, `:4635`); chain depth (`:4579`).
- An in-scope `observation` object has `scope_id` and is live, so step 1
  passes and step 2 refuses it; no test drives this.
- `load_subject_facts` in the Kernel filters `invalidated_commit_seq IS
  NULL` again (`admission.rs:2143`), a second liveness gate.

## Failure scenario

Removing step 2's kind check lets a disposition land on an observation
object, whose admission history is the classifier's (`lib.rs:15215-
15227`), and the row would carry the observation's `source_kind`. Removing
the chain-member scoping lets a caller cite an approval whose ancestor
lives in another project, which the chain reader would otherwise treat as
valid authority.

## Timing windows and dependencies

The writer lock serializes commits, so a retirement that commits first is
seen by step 2. Preview runs on a read snapshot and can judge a subject
that the next commit retires; the commit then answers `not_found`
(`memory-mark-command.ts:296-297` reports `refused/commit`).

## What a test must construct

- Subjects: retired decision, unadmitted decision, foreign-project
  decision, live in-scope observation, evidence object id, nonexistent
  id.
- Approvals: in scope valid; out of scope; stored approval (none cited)
  whose object was retired after the subject's last admission.
- Assert the refusal name and unchanged `admission_decisions` and
  `commit_log` counts for each.

## Investigation log

### Q: Which refusal does a retired stored approval produce?

- Sources examined: `scoped_object_state` and `load_object_state` (no
  liveness filter, so a retired approval's row is returned);
  `validate_approval`, which judges the retired approval invalid.
- Findings: the evaluator sees `approval_valid = false`. Whether the
  operation then answers `not_found` or lands as a denied relaxation
  depends on the event, and the event-specific outcome was not traced.
- Missing evidence: a trace of `validate_approval`'s answer through
  `evaluate_admission` for each disposition event, or the test listed
  above under "What a test must construct".
- Conclusion: unresolved, needs that trace.
