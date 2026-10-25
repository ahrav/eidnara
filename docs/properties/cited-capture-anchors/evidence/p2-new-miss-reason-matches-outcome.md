# p2-new-miss-reason-matches-outcome

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N5, D5 amended by OQ2-12, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

`Citation::lookup` (`crates/daemon/src/memory_capture/cite.rs:240`) returns `source_capture_off` without reading when the binding's `source_capture_enabled` or `memory_enabled` is off; otherwise it reads the message's blocks and, when none is live, returns `source_id_unjoinable` for a message id of the hashed shape (`is_hashed_source_id`, `:114`: 64 lowercase hex, `:`, canonical decimal) and `descriptor_not_found` otherwise. `publish_captured_memories` writes exactly one `citation` and one `anchor` value per decision and pushes the matching count. `publish_budget_expired` has a counter and a wire name but no producer until U4.

## Failure scenario

A decision without a citation and without a reason would make a timing defect indistinguishable from a missing source.

## Timing windows and dependencies

None.

## What a test must construct

Each reason constructed separately: `crates/daemon/src/source_capture_tests.rs:3475` (cited and descriptor not found), `crates/daemon/src/source_capture_tests.rs:3625` (source capture off, hashed id, plain, unborn, and SHA-256 roots, detached HEAD); the hashed fragment id the checkpoint writes passes the shape test (`crates/daemon/src/memory_capture.rs:1549`).

## Investigation log

### Q: Is `publish_budget_expired` constructible?

- Sources examined: the U3 code.
- Findings: No producer exists; U4 adds the settlement outcome.
- Missing evidence: the U4 settlement record.
- Conclusion: unresolved, needs #1136.
