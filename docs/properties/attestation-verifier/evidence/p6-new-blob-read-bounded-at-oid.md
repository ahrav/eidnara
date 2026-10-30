# p6-new-blob-read-bounded-at-oid

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N2. A proposed obligation this ticket converts to active with the checks it names.

## Evidence trail

`lookup` (`crates/daemon/src/attestation_verifier.rs:538`), called by `verify` (`:381`), polls `Stage::Open` before `git_sources::open_with_slots`, `Stage::Commit` before the commit, `Stage::Tree` before each tree, `Stage::Header` before `Objects::blob_header` (`:468`), and `Stage::Blob` before the one blob read. `Objects::read` (`:485`) decodes an object held in the repository's own packs through `OwnPacks::decode` (`crates/daemon/src/git_sources.rs:329`), whose `entry_header` walk (`:445`) follows ofs- and ref-delta bases with a hop bound of the index's entry count before the decode; an own-pack entry whose ofs- and ref-delta walk revisits an entry is refused before any decode; an id whose every holding own pack revisits an entry, names a base outside that pack, or has an undecodable header, or whose located entry fails the decode, reads a valid loose copy or is `object_unreadable(class)`; the object store never reads an id an own index holds. The packs decode with `MAX_BLOB_BYTES` as their allocation limit. An object no own index holds reads from the repository's loose store under the same limit, a loose read error is `object_unreadable(class)`, and an id no own index or loose file holds, such as one in an alternate, reads through the opened object store; a lookup there that exhausts the store's index slots makes at most one reopen attempt per claim through `git_sources::open`, and later lookups in that claim use the reopened store. `Objects::blob_header` takes the blob's header from the same own-pack walk, the loose header, or the store's header. A header whose size decides the verdict ends the walk as `Observed::Header` with no blob poll. `Objects::read` maps an allocation refusal to `over_budget`, adds the decoded length to its counter, and then refuses an object of another kind or whose bytes do not hash to its id, so a blob rejected after decode counts its decoded bytes in `bytes_read`; the commit and trees are read into a discarded counter. The commit and each tree decode in full, so a valid prefix with a malformed remainder is unreadable. Tests: `the_header_gates_every_blob_read` (`crates/daemon/tests/attestation_verifier.rs:557`), `a_blob_whose_delta_base_exceeds_the_allocation_limit_is_over_budget` (`:610`), `a_passed_deadline_opens_no_repository` (`:694`; a nonexistent root yields `deadline(open)` under a cancelled or expired `EvalBudget` and `repository_unopenable` without one), `a_deadline_before_the_header_poll_reads_no_blob` (`:720`), `missing_or_corrupt_objects_are_unreadable_never_absent` (`:382`; valid objects stored under another id, and a commit and a tree that hash to their ids but do not decode), `entries_that_hold_no_blob_are_named_by_kind` (`:340`), `the_driver_writes_nothing` (`:501`, with a positive control), `replacement_refs_are_ignored` (`:521`), `committed_bytes_match_and_a_later_commit_mismatches` (`:228`), `a_nested_path_walks_one_tree_per_component` (`:269`; one `Tree` poll per component), `a_directory_that_is_no_repository_is_unopenable` (`:491`), `a_delta_chain_that_revisits_an_entry_is_unreadable` (`:667`; a self-referencing ref-delta commit, tree, and blob), `a_blob_mode_entry_naming_another_kind_is_unreadable` (`:1028`; the header kind guard), `each_deadline_poll_precedes_its_read` (`:1053`; a deleted object behind each poll reports the deadline), and `claim_paths_follow_the_document_path_rule` (`:798`; the 16-component cap), `a_ref_delta_in_an_own_pack_decodes_against_its_base_in_that_pack` (`:1131`; a repacked ref-delta blob decodes from the own pack), and `objects_in_an_alternate_with_more_packs_than_index_slots_match` (`:1162`; loose objects in an alternate, then forty-one alternate packs, read through the object store), `a_delta_with_an_empty_body_that_names_itself_is_unreadable` (`:1213`), `an_own_pack_blob_whose_stream_does_not_decode_is_unreadable` (`:1240`), `a_tree_above_the_allocation_limit_is_over_budget_loose_and_packed` (`:1256`; the loose store's and the own packs' allocation limits), `a_blob_only_in_an_alternate_with_more_packs_than_index_slots_matches` (`:1285`; the header lookup's reopen), `an_own_pack_delta_whose_base_lies_in_another_pack_is_unreadable` (`:1323`), `a_valid_loose_copy_answers_for_an_own_pack_entry_that_does_not_resolve` (`:1374`), and `a_valid_own_pack_copy_answers_after_a_newer_pack_whose_chain_revisits_an_entry` (`:1394`). The cycle tests run `verify` through `verify_within` (`:1364`), which fails a call that runs past a minute. For one header and one blob read, the observations are the `Deadline` double's poll trace, one `Stage::Header` poll and one `Stage::Blob` poll per claim, and the `bytes_read` the driver returns, equal to the blob's length. These observe polls and counted reads; a read the driver made without a poll or outside `Objects::read` would leave both unchanged, which no test observes directly.

## Failure scenario

A decode before the header check, a worktree read, or a delta chain inflated without a limit.

## Timing windows and dependencies

The deadline crossing between object reads.

## What a test must construct

Blobs at and above 256 KiB, a hand-built pack with a delta over an oversized base, and deadline doubles that pass at each stage.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the allocation limit admit a blob of exactly 256 KiB?

- Sources examined: `the_header_gates_every_blob_read` (`crates/daemon/tests/attestation_verifier.rs:557`).
- Findings: Yes: a 262144-byte blob matches with `bytes_read == 262144`, and a 262145-byte blob is over budget with zero bytes read.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which reason does a commit or tree above the allocation limit take?

- Sources examined: `Objects::read` (`crates/daemon/src/attestation_verifier.rs:485`).
- Findings: `over_budget`: every object read maps an allocation refusal, from an own-pack decode, the loose store, or `exceeds_allocation` on the object store, to `over_budget`. `a_tree_above_the_allocation_limit_is_over_budget_loose_and_packed` (`crates/daemon/tests/attestation_verifier.rs:1256`) covers a tree above 256 KiB, loose and packed. No verifier test constructs a commit above 256 KiB, or either object on an oversized delta base.
- Missing evidence: none.
- Conclusion: resolved with answer; the oversized commit and the delta-base cases have no verifier test.
