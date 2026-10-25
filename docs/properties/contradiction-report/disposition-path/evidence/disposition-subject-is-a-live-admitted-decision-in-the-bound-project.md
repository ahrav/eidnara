# disposition-subject-is-a-live-admitted-decision-in-the-bound-project

Catalog record: [catalog.md](../catalog.md#disposition-subject-is-a-live-admitted-decision-in-the-bound-project).
HEAD `3a3b916b7`, 2026-10-25.

## Discovery trigger

Lens: security boundaries and lifecycle transitions. The route's
`not_found` doc (`state.rs:79-81`) names three refusal causes in one word;
the record lists the exact predicates a disposition subject must satisfy
and what each failure answers.

## Evidence trail

- `disposition_request` (`crates/daemon/src/kernel_routes/commit.rs:566-
  600`):
  1. `scoped_object_state(envelope, filter, object_id)?` (`:573`): the
     registry row exists and its `scope_id` names the bound project, else
     `NotFound`.
  2. `object_kind != "decision" || invalidated_commit_seq.is_some()` is
     `NotFound` (`:574-576`). `load_object_state` has no liveness filter
     of its own (`envelope.rs:1846-1860`), so this line is the liveness
     check.
  3. `subject_admission(object_id)?.ok_or(AdmissionPolicy)` (`:577-579`):
     a decision with no admission row is `AdmissionPolicy`.
  4. `approval_object_id.or(stored_approval)`: the approval and every
     `approval_chain_members` entry pass `scoped_object_state`
     (`:580-584`); the chain reader refuses above
     `MAX_AUTHORITY_CHAIN_DEPTH` with `AdmissionPolicy`
     (`admission.rs:1012-1033`).
- Mapping: `NotFound -> invalid/not_found`, `AdmissionPolicy ->
  invalid/admission_policy` (`kernel_routes/state.rs:275-276`).
- Both preview and commit call the same function (`commit.rs:634`,
  `:681`), so a refusal is identical in both modes
  (`kernel_routes.rs:5349`).
- Tests: retired subject and unadmitted subject (`kernel_routes.rs:4882-
  4935`); missing id (`:5072-5080`); foreign approval chains of three
  shapes (`:4562-4571`, `:4629`); chain depth (`:4573`).
- An in-scope `observation` object has `scope_id` and is live, so step 1
  passes and step 2 refuses it; no test drives this.
- `load_subject_facts` in the Kernel filters `invalidated_commit_seq IS
  NULL` again (`admission.rs:2138`), a second liveness gate.

## Failure scenario

Removing step 2's kind check lets a disposition land on an observation
object, whose admission history is the classifier's (`lib.rs:15050-
15062`), and the row would carry the observation's `source_kind`. Removing
the chain-member scoping lets a caller cite an approval whose ancestor
lives in another project, which the chain reader would otherwise treat as
valid authority.

## Timing windows and dependencies

The writer lock serializes commits, so a retirement that commits first is
seen by step 2. Preview runs on a read snapshot and can judge a subject
that the next commit retires; the commit then answers `not_found`
(`memory-mark-command.ts:142-143` reports `refused/commit`).

## What a test must construct

- Subjects: retired decision, unadmitted decision, foreign-project
  decision, live in-scope observation, evidence object id, nonexistent
  id.
- Approvals: in scope valid; out of scope; stored approval (none cited)
  whose object was retired after the subject's last admission.
- Assert the refusal name and unchanged `admission_decisions` and
  `commit_log` counts for each.

## Investigation log

No open questions. The retired-stored-approval case is listed as an
enabling state to construct, not as a question: `scoped_object_state` on
a retired approval returns the row (no liveness filter in
`load_object_state`), and `validate_approval` then judges it invalid;
whether the outcome is `not_found` or a denied relaxation depends on the
event and was not traced here.
