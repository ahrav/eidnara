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
| The new integration test file duplicated the broker fixture | refinement | fixed: the tests moved into `curator_broker.rs` and reuse its fixture |
| The `commit_token` oracle used the same formula as the implementation over a decision with one commit | gap | fixed: a second commit appends a decision event so the last change differs from creation |
| No coordinator-level witness exercised `resolve_subject` through `open` for a canonical subject | gap | fixed: `a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model` |
| The scheduler gate test derived its expectation from the gate constant, so flipping the gate changed nothing | gap | fixed: a constant assertion on the gate plus a fixed expected task list |
| Canonical before/after equality was asserted only on the Git path | gap | fixed: tracked registry rows compared before and after in both broker tests |
| The wrong-kind owner refusal at the broker is `Scope`, not a kind-specific code, because the evidence object is unscoped | bias | kept: the Kernel judges scope before kind; `proposal_target` supplies the kind refusal, and the evidence file records the ordering |
| `PRODUCTION_SELECTION_OPEN` is a compile-time constant rather than an operator switch | bias | kept: opening is a reviewed code change with its own witness, not a deployment action |

## Gaps queued

- A production-class positive witness cannot be constructed until an owner
  supplies a Remote-eligible decision representation. Queued under the owner of
  the policy-validating broker.
- Class tightening and wrong-scope decisions at the coordinator seam remain
  unconstructed; see `existing-checks.md`.
