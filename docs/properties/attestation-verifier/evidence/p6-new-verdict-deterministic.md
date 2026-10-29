# p6-new-verdict-deterministic

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N2. A proposed obligation this ticket converts to active with the checks it names.

## Evidence trail

`verify` (`crates/daemon/src/attestation_verifier.rs:353`) runs `observe` (`:403`), whose `lookup` (`:407`) opens through `git_sources::open` and reads by object id only; `observe` returns an `Observed` value on every path, and `verify` passes it to `verdict` (`:306`) once. `verdict` decides every verdict: it applies `header_verdict` (`:292`) to a header-only or content observation, then compares digests. `verdicts_survive_a_worktree_edit_a_head_move_and_a_repack` (`crates/daemon/tests/attestation_verifier.rs:641`) evaluates three claims, adds a linked worktree whose file it edits, moves `main`, repacks, and compares; the edited file's bytes differ from the claim, so a driver that discovered and read the linked worktree would change the verdict; the bare root has no workdir, so independence from a root's own worktree holds by construction, and a non-bare variant belongs with the #1173 caller. `the_fixture_has_no_worktree` (`:190`) asserts `is_bare` and no `workdir`. `committed_bytes_match_and_a_later_commit_mismatches` (`:200`) and `line_endings_normalize_in_either_direction` (`:264`) cover AC1; `replacement_refs_are_ignored` (`:477`) replaces the blob and the commit and still answers from the stored bytes; `the_pure_verdict_compares_digests_after_the_header_gate` (`:714`) and `normalized_digests_exist_only_for_a_reachable_length` (`:781`) drive `verdict` with observed-value doubles. The repack clause excludes one case: `over_budget` from a delta chain above the allocation limit depends on how the pack stores the blob, so a repack can change it.

## Failure scenario

`Match` then `Mismatch` at one oid after a worktree edit.

## Timing windows and dependencies

A worktree edit, a HEAD move, or a repack between two evaluations of one claim.

## What a test must construct

A bare repository, two commits, a linked worktree, a branch move, and a repack.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: How are line endings normalized?

- Sources examined: `Digests::of` (`crates/daemon/src/attestation_verifier.rs:230`) and the owner's OQ6-8 decision in the ticket.
- Findings: On a raw difference one candidate is computed: a claimed length above the blob's selects the CRLF form (every LF without a preceding CR becomes CRLF), a claimed length below selects the LF form (every CRLF becomes LF), and an equal length selects none, since a conversion that changes a byte changes the length.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the length shortcut hold as AC3 words it, a header size different from the claimed length being `Mismatch` with zero bytes read?

- Sources examined: `header_verdict` (`crates/daemon/src/attestation_verifier.rs:292`), AC1's normalized clause, and AC3's length-shortcut clause.
- Findings: AC1 and AC3 conflict. AC1 requires a CRLF-form claim against an LF blob to `Match`, and that claim's length differs from the header size; AC3 makes every header size different from the claimed length `Mismatch` with zero bytes read. The implementation keeps AC1: the shortcut decides `Mismatch` with zero bytes read only when no line-ending form can reach the claimed length (above twice the header size or below half of it); inside that range the blob is read and compared. This deviates from AC3's literal wording.
- Missing evidence: the owner's choice between AC1 and AC3's literal wording.
- Conclusion: needs human input

### Q: Is `over_budget` from an over-budget delta chain terminal? (needs human input)

- Sources examined: `Reason::is_terminal` (`crates/daemon/src/attestation_verifier.rs:174`), `read_object` (`:373`), the ticket's header-gate scope and its glossary's terminal split, and `a_blob_whose_delta_base_exceeds_the_allocation_limit_is_over_budget` (`crates/daemon/tests/attestation_verifier.rs:557`).
- Findings: The ticket maps a blob under the header budget whose delta base or summed deltas exceed the allocation limit to `over_budget`, and classes `over_budget` terminal. A header-gate `over_budget` is fixed by the blob's size at the oid. The delta case depends on the pack layout: the same blob stored whole or against a small base reads back, so a repack can turn the delta case into `Match` or `Mismatch`. The code keeps the ticket's mapping and terminal split and documents the exception; the owner of the terminal split in the bounded pass (#1173, U3) decides whether the delta case stays terminal.
- Missing evidence: the U3 owner's decision on the terminal split for the delta case.
- Conclusion: needs human input
