# p6-new-verdict-deterministic

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N2. A proposed obligation this ticket converts to active with the checks it names.

## Evidence trail

`verify` (`crates/daemon/src/attestation_verifier.rs:327`) runs `observe` (`:377`), which opens through `git_sources::open`, reads by object id only, and returns an `Observed` value or an early verdict; `verdict` (`:284`) compares digests after `header_verdict` (`:272`). `verdicts_survive_a_worktree_edit_a_head_move_and_a_repack` (`crates/daemon/tests/attestation_verifier.rs:575`) evaluates three claims, adds a linked worktree whose file it edits, moves `main`, repacks, and compares; the edited file's bytes differ from the claim, so a worktree read would change the verdict. `the_fixture_has_no_worktree` (`:183`) asserts `is_bare` and no `workdir`. `committed_bytes_match_and_a_later_commit_mismatches` (`:193`) and `line_endings_normalize_in_either_direction` (`:257`) cover AC1; `the_pure_verdict_compares_digests_after_the_header_gate` (`:648`) and `normalized_digests_exist_only_for_a_reachable_length` (`:704`) drive `verdict` with observed-value doubles.

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

- Sources examined: `Digests::of` (`crates/daemon/src/attestation_verifier.rs:223`) and the owner's OQ6-8 decision in the ticket.
- Findings: On a raw difference one candidate is computed: a claimed length above the blob's selects the CRLF form (every LF without a preceding CR becomes CRLF), a claimed length below selects the LF form (every CRLF becomes LF), and an equal length selects none, since a conversion that changes a byte changes the length.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the length shortcut hold as AC3 words it, a header size different from the claimed length being `Mismatch` with zero bytes read?

- Sources examined: `header_verdict` (`crates/daemon/src/attestation_verifier.rs:272`) and AC1's normalized clause.
- Findings: AC1 requires a CRLF-form claim against an LF blob to `Match`, and that claim's length differs from the header size. The shortcut decides `Mismatch` with zero bytes read when no line-ending form can reach the claimed length (above twice the header size or below half of it); inside that range the blob is read and compared.
- Missing evidence: none.
- Conclusion: resolved with answer; a deviation from AC3's literal wording, recorded for the owner.
