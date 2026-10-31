# p6-new-cursor-never-skips-unresolved

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. Converted to active by [#1173](https://github.com/ahrav/eidnara/issues/1173), which lands the bounded pass in `crates/daemon/src/attestation_verifier/bounded_pass.rs` and the Kernel page read `scope_decisions_after_within_budget` (`crates/kernel/src/slice/read.rs:833`).

## Evidence trail

The page read returns a project scope's decisions live at the pass's target, excluding `source_kind = "verifier"`, in `(created_commit_seq, object_id)` order after the cursor pair. A decision's creating commit never changes, so a cursor pair names one position in a fixed order. `ProjectRun::decisions` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:545`) moves the cursor to a decision only when that decision and every decision between the old cursor and it are resolved (`:618`); a resolved decision behind an unresolved one is remembered and not read again. A transient outcome returns `Unresolved` from `retry` (`:646`) until its K-th attempt. `ProjectRun::run` (`:526`) resets the state when the store's incarnation changes (`:533`). A project whose pages end, at an empty page or a page shorter than the row cap (`:571-572`), while every decision it read is resolved records just past the target's commit sequence as its floor (`:639`), and its next pass pages from the later of the floor and its cursor (`:547`); every later decision is created by a later commit, so the page passes over only resolved decisions, and a pass whose target precedes that page start reads no page (`:553-558`), since the page holds only decisions created from its start through the target. A restart starts with no cursor and no floor; the skips read the latest own admission (`:604-604`) and the registry for the attestation id at the anchored oid (`:683`), at the anchor the page's standing read names before the decision is classified and again after, before any claim verification read or Kernel write, so a rescan of attested or `Stale` decisions writes nothing.

`a_deadline_between_decisions_leaves_exactly_k_verdicts_and_the_cursor_before_the_next` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:553`) stops a pass after k = 3 decisions and finds 3 verdicts with the cursor at the third; a deadline during a blob read leaves the decision unresolved and unwritten, and the next pass verifies it. `a_transient_head_resolves_after_k_passes_and_a_terminal_reason_is_read_once` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:745`) keeps the cursor before an unbound head for K passes while the decisions behind it resolve once. `a_restart_rescan_writes_nothing_and_counts_skips` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:785`) reopens the store under a fresh verifier: the tip is unchanged, no repository opens, and the attested and `Stale` decisions count `skipped_attested` and `skipped_stale`. `a_drained_project_verifies_the_decisions_committed_after_its_last_page` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:2510`) reads each drained project's floor through `Verifier::floor`, then verifies a decision committed at the floor's own sequence, decisions committed behind another project's, and a decision committed during the project's own pass after its target capture, and keeps a project at its cursor while a transient decision committed past its floor resolves. `an_attested_decision_skips_before_its_classification_after_its_descriptor_retires` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:2619`) counts `skipped_attested` for an attested decision whose classification alone reads `descriptor_retired`. `a_terminal_reason_does_not_resolve_the_same_object_id_citing_another_path` (`:2729`) records `path_absent` for a decision in one store, then runs the same verifier over a second store whose decision under the same object id and oid cites a path the commit holds: the repository opens once and the attestation commits, so the terminal set is keyed by the decision, the claim's path, and its oid (`crates/daemon/src/attestation_verifier/bounded_pass.rs:769`), and an object id another history reuses for another path resolves nothing. `a_resolved_decision_retired_behind_a_transient_head_is_forgotten_at_the_floor` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:2831`) verifies a decision behind a `repository_unbound` head, so the pass remembers it above the cursor, then retires both: the next pass's empty page records the floor and drops the remembered decision and the head's attempt with it (`crates/daemon/src/attestation_verifier/bounded_pass.rs:638`), so a retired batch leaves no memory in the project's state. `retired_decisions_behind_a_transient_first_row_are_dropped` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:3600`) retires a transient head and the verified decision behind it, then commits another transient decision: the next pass's first row is unresolved, so no floor is recorded, and the visit still drops the retired attempt and the remembered decision before its key (`crates/daemon/src/attestation_verifier/bounded_pass.rs:592-593`).

## Failure scenario

A `deadline` decision passed and never revisited, or a second `MarkStale` after a restart.

## Timing windows and dependencies

A deadline between decisions or during a read; a restart between passes.

## What a test must construct

F7: a deadline between decisions k and k + 1 and during a read; F13: a transient head; a restart.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1174 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: What are N and K?

- Sources examined: OQ6-10 and its resolution in [#1173](https://github.com/ahrav/eidnara/issues/1173).
- Findings: N = 8 verdicts per project per pass and K = 3 attempts (`crates/daemon/src/attestation_verifier/bounded_pass.rs:100`, `:102`).
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Why is the cursor a pair rather than a commit sequence alone?

- Sources examined: the page read's order and `ProjectRun::decisions`.
- Findings: One commit can create several decisions, and N or the deadline can stop a pass between two of them; the object id orders decisions inside one commit so the cursor can stop there.
- Missing evidence: none.
- Conclusion: resolved with answer.
