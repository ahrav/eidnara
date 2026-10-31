# p6-new-verifiability-pure-function-of-snapshot

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N1. A proposed obligation that [#1171](https://github.com/ahrav/eidnara/issues/1171) converts to active with the classifier, its two Kernel reads, and the `KernelStore::read_at` transaction that runs them.

## Evidence trail

`classify` (`crates/daemon/src/attestation_verifier/classify.rs:295`) takes a `CommitReadTarget`, the bound roots, and an `EvalBudget`, and returns a `Classification` that carries the target. It runs `class_of` (`:316`) inside one `KernelStore::read_at` transaction (`:302`; `crates/kernel/src/snapshot_read.rs:34`), which takes a reader under the budget, checks the sequence against the tip, and compares the store's incarnation with `target.incarnation` under that reader guard (`crates/kernel/src/snapshot_read.rs:44`) before `class_of` makes its first read. A restore locks the writer and then every reader (`crates/kernel/src/backup.rs:564`, `:567-576`) and advances the restore generation while it holds them (`:689-692`), so none lands between two reads of one classification. `class_of` makes five kinds of read through the transaction's `SnapshotRead`, each at `target.through_commit`:

- `decision_standing`, called at `crates/daemon/src/attestation_verifier/classify.rs:321` and defined at `crates/kernel/src/claim_facts.rs:1005`: the registry row, decision row, and latest own admission at the sequence, with the row checks of `claim_standing_at_within_budget`, and the liveness of the decision row's own evidence at the sequence.
- `served_facts`, called at `crates/daemon/src/attestation_verifier/classify.rs:340` and defined at `crates/kernel/src/claim_facts.rs:1092`: the class the serving view serves the decision at the sequence, or no row.
- `anchor_condition`, called at `crates/daemon/src/attestation_verifier/classify.rs:352-353` and defined at `crates/kernel/src/anchor_write.rs:488`, the read `anchor_condition_as_of` (`:469`) runs: the anchor row live at the sequence, decoded by `validate_anchor_row` (`:288`).
- `linked_decision_evidence`, called in `citations` (`crates/daemon/src/attestation_verifier/classify.rs:410-411`) and defined at `crates/kernel/src/slice/read.rs:787`: the event-level citations at the sequence, each with its evidence liveness at the sequence.
- `source_descriptors_citing_evidence`, called at `crates/daemon/src/attestation_verifier/classify.rs:373` and defined at `crates/kernel/src/source_descriptor.rs:1300`, the statement `source_descriptor_citing_evidence` (`:1230`) runs for one id: the document descriptor citing each cited evidence row, live at the sequence.

After the transaction, whether it succeeded or refused, `classify` compares the store's incarnation with `target.incarnation` again (`crates/daemon/src/attestation_verifier/classify.rs:308-309`) and returns `IncarnationMismatch` when they differ. Every read already ran under the guarded comparison, so this second comparison matters for refusals the transaction raises ahead of it or in its place, a cancelled budget or a target past the tip; a restore that completes after it changes no read. The bound roots are an argument; the classifier maps the descriptor's `repository_id` to the first root with that identity and carries the root path, so it opens no repository.

The budget bounds availability, and the guarantee covers the `Class` of a classification that completes. `read_as_of` checks the budget before the first read (`crates/kernel/src/snapshot_read.rs:58`) and the Kernel reads check it as they run; `EvalBudget::is_exhausted` (`crates/kernel/src/applicability/checkout.rs:203`) reads `Instant::now()` when the budget carries a deadline. Two calls with equal inputs can therefore return a classification before the deadline and `Deadline` after it. The `Class` a completed call returns is read from the snapshot and the bound roots the caller passes; the budget's deadline or cancellation decides only whether the reads complete. The source scan in `classification_at_one_sequence_ignores_the_worktree_head_and_later_commits` covers `classify.rs`, which names no clock; the budget is an argument, so the scan does not see this dependency, and the record states it here.

The checks are `classification_at_one_sequence_ignores_the_worktree_head_and_later_commits` (`crates/daemon/tests/attestation_verifier_classify.rs:950`), which classifies four decisions twice at one target around a worktree edit, a commit on an orphan branch that moves the bound checkout's HEAD so the anchor oid is unreachable from it, a dirty worktree file, and Kernel changes after the target that retire one decision's cited descriptor, invalidate another's anchor by SQL, and delete the evidence the third cites at event level and the fourth at row level through `delete_artifact`; it asserts the two classifications of each decision equal and every one verifiable, and asserts the tip classifications are `descriptor_retired`, `no_anchor`, `evidence_retired`, and `evidence_retired` as positive controls that the later changes are observable. Its scan of the classifier's source for git, filesystem, clock, process, and network APIs is a backstop; the behavioral assertions carry the claim. `a_decision_outside_the_input_set_counts_under_no_reason` (`:805`) classifies a decision retired and one marked `Stale` after an earlier target and finds both verifiable at that target. `a_target_of_another_incarnation_is_refused` (`:713`) classifies with a target whose incarnation names another store and gets `IncarnationMismatch`, and still gets it when the target is past the tip or the budget is cancelled, so the mismatch takes precedence over an earlier refusal. `read_at_refuses_before_and_after_its_closure_runs` (`crates/kernel/tests/kernel_snapshot_read.rs:276`) gets `IncarnationMismatch`, `FutureSnapshot`, `InvalidInput`, and `Deadline` from `read_at` with a closure that panics if it runs, and `Deadline` for a budget cancelled inside a closure that returns a result or a refusal. `the_decision_standing_equals_the_standing_read_without_its_serving_facts_at_every_snapshot` (`:143`) and `one_descriptor_statement_answers_each_cited_row_as_its_single_call_read` (`:198`) compare the standing and descriptor reads in `read_at` with `claim_standing_at_within_budget`, `evidence_liveness_at_within_budget`, and the single-call descriptor read at every sequence of a fixture store.

## Failure scenario

A classifier that read HEAD, the worktree, or the clock to decide a class, or that read any Kernel row at the tip instead of the requested sequence, would classify one decision differently before and after an unrelated commit, and the write set's recheck at the classification's target would compare against another snapshot.

## Timing windows and dependencies

The reads run in one deferred reader transaction, so they see one database state. Each also reads rows as of the requested sequence: a row's creation sequence never changes, and each read compares the lifecycle columns a later commit updates in place, such as `invalidated_commit_seq` and `superseded_by`, with the requested sequence. A restore waits for every reader guard, so it lands before the transaction's guarded incarnation comparison, which refuses the target, or after the transaction ends, when every read already saw the target's history. The classification holds one pooled reader from acquisition to commit, for at most seven read statements: the tip, the standing, the approval check when an approval exists, the anchor, the linked evidence's decision lookup and event walk, and the descriptor statement over at most 33 cited rows. A restore waits that long, and so do every writer behind it and every reader of the slots the restore already holds.

## What a test must construct

Two classifications of one decision at one `CommitReadTarget`, with a worktree edit, a HEAD move, and later Kernel commits that change the tip classification between them; a target of another incarnation.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the #1174 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is a restore between two of the classifier's reads detected?

- Sources examined: `classify`, `KernelStore::read_at`, `KernelStore::read_as_of`, `KernelStore::require_incarnation`, and the restore's guard acquisition in `crates/kernel/src/backup.rs` at the branch tip.
- Findings: Every read of one classification runs in the `read_at` transaction, which compares the incarnation under its reader guard before the first read (`crates/kernel/src/snapshot_read.rs:44`). A restore locks every reader before it advances the restore generation (`crates/kernel/src/backup.rs:567-576`, `:689-692`), so no restore lands between two reads; one that lands before the comparison is refused there, and `classify` compares the incarnation again after the transaction (`crates/daemon/src/attestation_verifier/classify.rs:308-309`), so a refusal raised ahead of the guarded comparison, or a `Deadline` raised in its place, still reports it.
- Missing evidence: a test that starts a restore while a classification holds its reader; `read_at_refuses_before_and_after_its_closure_runs` covers the refusal of a target of another incarnation before the closure runs, and the guard's position rests on reading `read_at`.
- Conclusion: resolved with answer: yes.
