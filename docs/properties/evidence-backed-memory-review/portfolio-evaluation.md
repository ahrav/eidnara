# Portfolio evaluation

The records in this part were reconstructed from the specification and the
code, so no lens passes produced them and no fresh-context evaluation of a
discovery portfolio ran. In its place, each change that introduces records is
reviewed before its pull request opens by independent reviewers that have not
seen the design reasoning, through five lenses: over-engineering, complexity,
testing, Rust code review, and Rust design review. This file records what those
reviews found and how each finding was dispositioned.

## Production canonical resolution

| Finding | Class | Disposition |
| --- | --- | --- |
| `Run.subject` was an `Option` that was `None` only before `open`, with an unreachable refusal arm | refinement | fixed: the subject is an owned local returned from the opening block and passed to `bind_proposal` |
| `proposal_target` accepted a decision target whose registry row is not a decision; the coordinator was safe only because `open` ran the broker first | gap | fixed: the target refuses `ExpectationChanged` when the row's `object_kind` is not `decision`; test asserts it |
| Two refusal codes for one malformed-row class inside `resolve_descriptor` | refinement | fixed: a missing identity field refuses `Unsupported` like the sibling decode checks |
| Docs on `resolve_descriptor`, `bind_proposal`, `MEMORY_CLASSES`, and `originating_decision` dropped the contract the old text stated | refinement | fixed: contracts restored naming each refusal and the `Refused`-vs-`Kernel` split |
| `related_memories` decoded the class twice for one row | refinement | fixed: `expectation` calls `originating_decision` with the class it already parsed |
| The new integration test file duplicated the broker fixture | refinement | fixed: the tests moved into `memory_reviewer_broker.rs` and reuse its fixture |
| The `commit_token` oracle used the same formula as the implementation over a decision with one commit | gap | fixed: a second commit appends a decision event so the last change differs from creation |
| No coordinator-level witness exercised `resolve_subject` through `open` for a canonical subject | gap | fixed: `a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model` |
| The scheduler gate test derived its expectation from the gate constant, so flipping the gate changed nothing | gap | fixed: a constant assertion on the gate plus a fixed expected task list |
| Canonical before/after equality was asserted only on the Git path | gap | fixed: tracked registry rows compared before and after in both broker tests |
| The wrong-kind owner refusal at the broker is `Scope`, not a kind-specific code, because the evidence object is unscoped | bias | kept: the Kernel judges scope before kind; `proposal_target` supplies the kind refusal, and the evidence file records the ordering |
| `PRODUCTION_SELECTION_OPEN` is a compile-time constant rather than an operator switch | bias | kept: opening is a reviewed code change with its own witness, not a deployment action |

## Private result expiry

| Finding | Class | Disposition |
| --- | --- | --- |
| A reconciliation error returned before Kernel maintenance, so one unparsable pin could starve capture expiry and staging cleanup for up to seven days | gap | fixed: the pass records `healthy = false` and continues into Kernel maintenance; an unparsable owner is skipped rather than failing the listing |
| `in_progress_memory_reviewer_receipts` took a `LIMIT` ordered by run deadline, so overflow would omit the newest receipts and release holds their settlements still needed | gap | fixed: the query is unbounded; every in-progress receipt holds a pending job under the store's own bounds |
| `memory_reviewer_result_is_selected` ignored the selection's project digest and store incarnation, so a same-target job elsewhere could keep a hold alive | gap | fixed: both predicates added from the hold's binding |
| Memory Store reads were interleaved with Kernel releases inside one loop | refinement | fixed: orphans are collected first, then released |
| `DeadlineClock` enum encoded one integer floor | refinement | fixed: `load_staged_review` takes the instant directly |
| `read_selected_review_input` carried an unused `now`; `ActiveReviewHold` carried an unread `expires_at` | refinement | fixed: both removed |
| `updated_at_ms` is not covered by the receipts' immutability trigger, so `completed_at_ms` rests on `WHERE state = 'in_progress'` clauses | bias | kept: every write site is gated on the in-progress state today; a trigger or dedicated column would change the Memory Store baseline and belongs with the owner's next baseline change |
| Capture `retain_until` still moves to the review expiry at transfer | bias | kept: a retention floor, not a visibility path; recorded as an open question on the record |
| The selected read's owner check moved from the Kernel to the daemon and had no witness; the reconciler's generation and project scoping had none either | gap | fixed: tests mutate the stored owner and class after selection, orphan a hold through takeover, and query the selection question across digests and generations |
| `completed_at_ms` was asserted only as earlier than the deadline, and no test swept after a completed selection | gap | fixed: exact completion time asserted and re-asserted after a sweep at the queue deadline |
| No test placed the sweep inside the settlement window | gap | fixed: `before_completion_for_test` runs the sweep; the completion is fenced and the settlement releases the hold |
| No test reopens the stores between envelope and sweep | bias | kept: the rows are SQLite writes already committed; a reopen witness is queued under the U1 qualification owner |

## Gaps queued

- A production-class positive witness cannot be constructed until an owner
  supplies a Remote-eligible decision representation. Queued under the owner of
  the policy-validating broker.
- Class tightening and wrong-scope decisions at the coordinator seam remain
  unconstructed; see `existing-checks.md`.
