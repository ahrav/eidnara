# p6-new-blob-read-bounded-at-oid

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N2. A proposed obligation this ticket converts to active with the checks it names.

## Evidence trail

`lookup` (`crates/daemon/src/attestation_verifier.rs:407`), called by `observe` (`:403`), polls `Stage::Open` before `git_sources::open`, `Stage::Commit` before the commit, `Stage::Tree` before each tree, `Stage::Header` before `try_find_header`, and `Stage::Blob` before the one `find_object` on the blob. A header whose size decides the verdict ends the walk as `Observed::Header` with no blob poll. `read_object` (`:373`) maps `exceeds_allocation` to `over_budget`, records the decoded length, and then refuses an object of another kind or whose bytes do not hash to its id, so a blob rejected after decode counts its decoded bytes in `bytes_read`. Tests: `the_header_gates_every_blob_read` (`crates/daemon/tests/attestation_verifier.rs:504`), `a_blob_whose_delta_base_exceeds_the_allocation_limit_is_over_budget` (`:557`), `a_passed_deadline_opens_no_repository` (`:595`; a nonexistent root yields `deadline(open)` under a cancelled or expired `EvalBudget` and `repository_unopenable` without one), `a_deadline_before_the_header_poll_reads_no_blob` (`:621`), `missing_or_corrupt_objects_are_unreadable_never_absent` (`:354`; valid objects stored under another id, and a commit and a tree that hash to their ids but do not decode), `entries_that_hold_no_blob_are_named_by_kind` (`:312`), `the_driver_writes_nothing` (`:457`, with a positive control), `replacement_refs_are_ignored` (`:477`), and `committed_bytes_match_and_a_later_commit_mismatches` (`:200`). For one header and one blob read, the observations are the `Deadline` double's poll trace, one `Stage::Header` poll and one `Stage::Blob` poll per claim, and the `bytes_read` the driver returns, equal to the blob's length.

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

- Sources examined: `the_header_gates_every_blob_read` (`crates/daemon/tests/attestation_verifier.rs:504`).
- Findings: Yes: a 262144-byte blob matches with `bytes_read == 262144`, and a 262145-byte blob is over budget with zero bytes read.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which reason does a commit or tree above the allocation limit take?

- Sources examined: `read_object` (`crates/daemon/src/attestation_verifier.rs:373`).
- Findings: `over_budget`: every object read maps an allocation refusal through `exceeds_allocation`. No test constructs a tree above 256 KiB.
- Missing evidence: none.
- Conclusion: resolved with answer; the tree case has no test.
