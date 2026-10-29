# p6-new-verifiability-pure-function-of-snapshot

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N1. A proposed obligation that [#1171](https://github.com/ahrav/eidnara/issues/1171) converts to active with the classifier and its two Kernel reads.

## Evidence trail

`classify` (`crates/daemon/src/attestation_verifier/classify.rs:186`) takes a `CommitReadTarget`, the bound roots, and an `EvalBudget`, and returns a `Classification` whose `commit_seq` is `target.through_commit`. `class_of` (`:199`) makes four kinds of Kernel read, each at that sequence and under that budget:

- `claim_standing_at_within_budget` (`crates/daemon/src/attestation_verifier/classify.rs:209`): the registry row, decision row, and latest own admission at the sequence, under the reader guard that compares the store's incarnation with the target's.
- `anchor_condition_at`, called at `crates/daemon/src/attestation_verifier/classify.rs:246` and defined at `crates/kernel/src/anchor_write.rs:468`: the anchor row live at the sequence, decoded by `validate_anchor_row` (`:287`).
- `linked_decision_evidence_within_budget` (`crates/daemon/src/attestation_verifier/classify.rs:254`): the row-level and event-level citations at the sequence, each with its evidence liveness at the sequence.
- `source_descriptor_citing_evidence`, called at `crates/daemon/src/attestation_verifier/classify.rs:266` and defined at `crates/kernel/src/source_descriptor.rs:1213`: the document descriptor citing each cited evidence row, live at the sequence.

The bound roots are an argument; the classifier maps the descriptor's `repository_id` to the first root with that identity and carries the root path, so it opens no repository. The module imports no git, filesystem, clock, process, or network API.

The checks are `classification_at_one_sequence_ignores_the_worktree_head_and_later_commits` (`crates/daemon/tests/attestation_verifier_classify.rs:668`), which classifies one decision twice at one target around a worktree edit, a commit that moves the bound checkout's HEAD, a dirty worktree file, and a Kernel commit that retires the cited descriptor, asserts the two classifications equal, asserts the tip classification is `descriptor_retired` as a positive control that the later commit is observable, and scans the classifier's source for those APIs; and `a_decision_outside_the_input_set_counts_under_no_reason` (`:550`), which classifies a decision retired and one marked `Stale` after an earlier target and finds both verifiable at that target.

## Failure scenario

A classifier that read HEAD, the worktree, or the clock, or that read any Kernel row at the tip instead of the requested sequence, would classify one decision differently before and after an unrelated commit, and the write set's recheck at the classification's sequence would compare against another snapshot.

## Timing windows and dependencies

The four reads run in separate reader transactions. Each reads rows as of the requested sequence, and committed rows at or below a sequence never change, so the transactions agree whatever commits between them. A restore between them could pair two histories; the standing read compares the incarnation under its reader guard, and the later reads take a sequence only.

## What a test must construct

Two classifications of one decision at one `CommitReadTarget`, with a worktree edit, a HEAD move, and a later Kernel commit that changes the tip classification between them.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the #1171 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the later reads check the store incarnation?

- Sources examined: `anchor_condition_at`, `linked_decision_evidence_within_budget`, and `source_descriptor_citing_evidence` at the branch tip.
- Findings: They take a sequence and check it against the tip; they do not compare an incarnation. `claim_standing_at_within_budget` does, first.
- Missing evidence: a test that restores a backup between the standing read and a later read.
- Conclusion: unresolved, needs that test or an incarnation check in the later reads.
