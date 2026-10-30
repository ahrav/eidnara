# p6-new-verdict-deterministic

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N2. A proposed obligation this ticket converts to active with the checks it names.

## Evidence trail

`verify` (`crates/daemon/src/attestation_verifier.rs:381`) runs `lookup` (`:538`), which opens through `git_sources::open_with_slots` and reads by object id only, from the repository's own packs, its loose store, or the object store; `verify` maps a refusal to `Observed::Unavailable`, so it holds an `Observed` value on every path, and `verify` passes it to `verdict` (`:329`) once. `verdict` decides every verdict: it applies `header_verdict` (`:313`) to a header-only or content observation, then compares digests. `verdicts_survive_a_worktree_edit_a_head_move_and_a_repack` (`crates/daemon/tests/attestation_verifier.rs:724`) evaluates three claims, adds a linked worktree whose file it edits, moves `main`, repacks, and compares; the edited file's bytes differ from the claim, so a driver that discovered and read the linked worktree would change the verdict. `a_non_bare_root_answers_from_the_object_store_not_the_worktree` (`:1071`) edits and stages the file in a non-bare root, and the claim at the commit still matches the committed bytes. A ref name or an abbreviated id is refused as `object_unreadable(commit)`, so a HEAD move cannot change what an oid names. `the_fixture_has_no_worktree` (`:202`) asserts `is_bare` and no `workdir`. `committed_bytes_match_and_a_later_commit_mismatches` (`:212`) and `line_endings_normalize_in_either_direction` (`:276`) cover AC1; `replacement_refs_are_ignored` (`:505`) replaces the blob and the commit, sets `core.useReplaceRefs = false` so a plain gix open reads the replacement, and still answers from the stored bytes; `a_raw_digest_match_needs_the_claimed_length` (`:951`) shows a raw digest match under a wrong claimed length is `Mismatch`; `an_empty_committed_file_matches_an_empty_claim` (`:970`) reads a zero-byte blob; `the_pure_verdict_compares_digests_after_the_header_gate` (`:805`) and `normalized_digests_exist_only_for_a_reachable_length` (`:879`) drive `verdict` with observed-value doubles. The repack clause excludes one case: `over_budget` from the allocation limit depends on how the pack stores the commit, a tree on the path, or the blob, so a repack can change it. The transient reasons can also differ between two evaluations of one store when the environment fails, for example when file descriptors run out.

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

- Sources examined: `Digests::of` (`crates/daemon/src/attestation_verifier.rs:245`) and the owner's OQ6-8 decision in the ticket.
- Findings: `Digests::of` computes the raw digest and at most one candidate, independent of the raw comparison: when the claimed length equals the content length plus its count of LFs without a preceding CR, the CRLF form (every such LF becomes CRLF); when it equals the content length minus its CRLF count, the LF form (every CRLF becomes LF); every other claimed length, including an equal one, yields no candidate. `header_verdict` decides `Mismatch` before any read only when the claimed length is above twice or below half the header size. A raw match also requires the header size to equal the claimed length.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the length shortcut hold as AC3 words it, a header size different from the claimed length being `Mismatch` with zero bytes read?

- Sources examined: `header_verdict` (`crates/daemon/src/attestation_verifier.rs:313`), AC1's normalized clause, and AC3's length-shortcut clause.
- Findings: AC1 and AC3 conflict. AC1 requires a CRLF-form claim against an LF blob to `Match`, and that claim's length differs from the header size; AC3 makes every header size different from the claimed length `Mismatch` with zero bytes read. The implementation keeps AC1: the shortcut decides `Mismatch` with zero bytes read only when no line-ending form can reach the claimed length (above twice the header size or below half of it); inside that range the blob is read and compared. This deviates from AC3's literal wording.
- Missing evidence: the owner's choice between AC1 and AC3's literal wording.
- Conclusion: needs human input

### Q: Is `over_budget` from the allocation limit terminal?

- Sources examined: `Reason::is_terminal` (`crates/daemon/src/attestation_verifier.rs:187`), `Objects::read` (`:485`), the ticket's header-gate scope and its glossary's terminal split, and `a_blob_whose_delta_base_exceeds_the_allocation_limit_is_over_budget` (`crates/daemon/tests/attestation_verifier.rs:594`).
- Findings: The ticket maps a blob under the header budget whose delta base or summed deltas exceed the allocation limit to `over_budget`, and classes `over_budget` terminal. A header-gate `over_budget` is fixed by the blob's size at the oid. The delta case depends on the pack layout: the same blob stored whole or against a small base reads back, so a repack can turn the delta case into `Match` or `Mismatch`. The same holds for the commit and for every tree on the path, since `Objects::read` maps their allocation refusals to `over_budget` too. The code keeps the ticket's mapping and terminal split and documents the exception. The #1173 ticket's Reasons list makes `over_budget` terminal, and the bounded pass keeps a terminal reason per `(decision, oid)` for the process lifetime (`crates/daemon/src/attestation_verifier/bounded_pass.rs:509`), so a repack that would change a delta-case `over_budget` is read again only after a restart.
- Missing evidence: none.
- Conclusion: resolved with answer: terminal, as the #1173 ticket sets.
