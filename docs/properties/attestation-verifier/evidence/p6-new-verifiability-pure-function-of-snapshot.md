# p6-new-verifiability-pure-function-of-snapshot

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N1. A proposed obligation that [#1171](https://github.com/ahrav/eidnara/issues/1171) converts to active with the classifier and its two Kernel reads.

## Evidence trail

`classify` (`crates/daemon/src/attestation_verifier/classify.rs:247`) takes a `CommitReadTarget`, the bound roots, and an `EvalBudget`, and returns a `Classification` that carries the target. `class_of` (`:266`) makes five kinds of Kernel read, each at `target.through_commit` and under that budget:

- `claim_standing_at_within_budget` (`crates/daemon/src/attestation_verifier/classify.rs:276`): the registry row, decision row, and latest own admission at the sequence, under the reader guard that compares the store's incarnation with the target's.
- `anchor_condition_at`, called at `crates/daemon/src/attestation_verifier/classify.rs:299` and defined at `crates/kernel/src/anchor_write.rs:468`: the anchor row live at the sequence, decoded by `validate_anchor_row` (`:287`).
- `evidence_liveness_at_within_budget` (`crates/daemon/src/attestation_verifier/classify.rs:361`): the liveness of the decision row's own evidence at the target, under a reader guard that compares the incarnation.
- `linked_decision_evidence_within_budget` (`crates/daemon/src/attestation_verifier/classify.rs:376`): the event-level citations at the sequence, each with its evidence liveness at the sequence.
- `source_descriptor_citing_evidence`, called at `crates/daemon/src/attestation_verifier/classify.rs:409` and defined at `crates/kernel/src/source_descriptor.rs:1213`: the document descriptor citing each cited evidence row, live at the sequence.

After the last read, whether it succeeded or refused, `classify` compares the store's incarnation with `target.incarnation` again (`crates/daemon/src/attestation_verifier/classify.rs:258`) and returns `IncarnationMismatch` when they differ. The bound roots are an argument; the classifier maps the descriptor's `repository_id` to the first root with that identity and carries the root path, so it opens no repository.

The checks are `classification_at_one_sequence_ignores_the_worktree_head_and_later_commits` (`crates/daemon/tests/attestation_verifier_classify.rs:896`), which classifies four decisions twice at one target around a worktree edit, a commit on an orphan branch that moves the bound checkout's HEAD so the anchor oid is unreachable from it, a dirty worktree file, and Kernel changes after the target that retire one decision's cited descriptor, invalidate another's anchor by SQL, and delete the evidence the third cites at event level and the fourth at row level through `delete_artifact`; it asserts the two classifications of each decision equal and every one verifiable, and asserts the tip classifications are `descriptor_retired`, `no_anchor`, `evidence_retired`, and `evidence_retired` as positive controls that the later changes are observable. Its scan of the classifier's source for git, filesystem, clock, process, and network APIs is a backstop; the behavioral assertions carry the claim. `a_decision_outside_the_input_set_counts_under_no_reason` (`:777`) classifies a decision retired and one marked `Stale` after an earlier target and finds both verifiable at that target. `a_target_of_another_incarnation_is_refused` (`:685`) classifies with a target whose incarnation names another store and gets `IncarnationMismatch`.

## Failure scenario

A classifier that read HEAD, the worktree, or the clock, or that read any Kernel row at the tip instead of the requested sequence, would classify one decision differently before and after an unrelated commit, and the write set's recheck at the classification's target would compare against another snapshot.

## Timing windows and dependencies

The reads run in separate reader transactions. Each reads rows as of the requested sequence, and committed rows at or below a sequence never change, so the transactions agree whatever commits between them. A restore between two of them could pair two histories. A restore only advances the store's restore generation, so the comparison after the last read detects a restore between any two reads, and a restore after that comparison changes no read.

## What a test must construct

Two classifications of one decision at one `CommitReadTarget`, with a worktree edit, a HEAD move, and later Kernel commits that change the tip classification between them; a target of another incarnation.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the #1171 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is a restore between two of the classifier's reads detected?

- Sources examined: `classify`, `anchor_condition_at`, `linked_decision_evidence_within_budget`, `source_descriptor_citing_evidence`, and `KernelStore::require_incarnation` at the branch tip.
- Findings: The anchor, event-citation, and descriptor reads take a sequence and check it against the tip. The standing read and the row-level liveness read compare the incarnation under their reader guards, and `classify` compares it after the last read (`crates/daemon/src/attestation_verifier/classify.rs:258`). The restore generation only advances, so that comparison sees any restore that landed before it.
- Missing evidence: a test that restores a backup between two reads; `classify` has no seam between its reads, so the test covers the refusal of a target of another incarnation.
- Conclusion: resolved with answer: yes.
