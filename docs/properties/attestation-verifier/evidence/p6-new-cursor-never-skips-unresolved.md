# p6-new-cursor-never-skips-unresolved

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. Converted to active by [#1173](https://github.com/ahrav/eidnara/issues/1173), which lands the bounded pass in `crates/daemon/src/attestation_verifier/bounded_pass.rs` and the Kernel page read `scope_decisions_after_within_budget` (`crates/kernel/src/slice/read.rs:701`).

## Evidence trail

The page read returns a project scope's decisions live at the pass's target, excluding `source_kind = "verifier"`, in `(created_commit_seq, object_id)` order after the cursor pair. A decision's creating commit never changes, so a cursor pair names one position in a fixed order. `ProjectRun::run` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:386`) moves the cursor to a decision only when that decision and every decision between the old cursor and it are resolved (`:446`); a resolved decision behind an unresolved one is remembered and not read again. A transient outcome returns `Unresolved` from `retry` (`:464`) until its K-th attempt. The state is reset when the store's incarnation changes (`:393`). A restart starts with no cursor; the skips read the latest own admission (`:434`) and the registry for the attestation id (`:519`) before any repository read or write, so a rescan of attested or `Stale` decisions writes nothing.

`a_deadline_between_decisions_leaves_exactly_k_verdicts_and_the_cursor_before_the_next` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:538`) stops a pass after k = 3 decisions and finds 3 verdicts with the cursor at the third; a deadline during a blob read leaves the decision unresolved and unwritten, and the next pass verifies it. `a_transient_head_resolves_after_k_passes_and_a_terminal_reason_is_read_once` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:729`) keeps the cursor before an unbound head for K passes while the decisions behind it resolve once. `a_restart_rescan_writes_nothing_and_counts_skips` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:769`) reopens the store under a fresh verifier: the tip is unchanged, no repository opens, and the attested and `Stale` decisions count `skipped_attested` and `skipped_stale`.

## Failure scenario

A `deadline` decision passed and never revisited, or a second `MarkStale` after a restart.

## Timing windows and dependencies

A deadline between decisions or during a read; a restart between passes.

## What a test must construct

F7: a deadline between decisions k and k + 1 and during a read; F13: a transient head; a restart.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1173 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: What are N and K?

- Sources examined: OQ6-10 and its resolution in [#1173](https://github.com/ahrav/eidnara/issues/1173).
- Findings: N = 8 verdicts per project per pass and K = 3 attempts (`crates/daemon/src/attestation_verifier/bounded_pass.rs:60`, `:62`).
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Why is the cursor a pair rather than a commit sequence alone?

- Sources examined: the page read's order and `ProjectRun::run`.
- Findings: One commit can create several decisions, and N or the deadline can stop a pass between two of them; the object id orders decisions inside one commit so the cursor can stop there.
- Missing evidence: none.
- Conclusion: resolved with answer.
