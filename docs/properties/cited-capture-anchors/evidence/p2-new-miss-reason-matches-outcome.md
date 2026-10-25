# p2-new-miss-reason-matches-outcome

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N5, D5 amended by OQ2-12, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) and completed by the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).

## Evidence trail

`Citation::lookup` (`crates/daemon/src/memory_capture/cite.rs:260`) returns `source_capture_off` without reading when the binding's `source_capture_enabled` or `memory_enabled` is off; otherwise it reads the message's blocks and, when none is live, returns `source_id_unjoinable` for a message id of the hashed shape (`is_hashed_source_id`, `:132`: 64 lowercase hex, `:`, canonical decimal) and `descriptor_not_found` otherwise. `publish_captured_memories` writes exactly one `citation` and one `anchor` value per decision and pushes the matching count. Since U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136)) the submit passes a `SettlementOutcome` (`crates/daemon/src/source_capture/settlement.rs:55`) into `publish`: consent off gives `Off`, which yields `source_capture_off`; an empty read after `BudgetExpired` (a refused handoff, an `Ended { Deadline }` entry, or a wait that hit `accepted_at + 5 s`) yields `publish_budget_expired`; an empty read after `Settled` yields `descriptor_not_found`. The hashed-shape test runs first, so a hashed id stays `source_id_unjoinable` under any outcome.

## Failure scenario

A decision without a citation and without a reason would make a timing defect indistinguishable from a missing source.

## Timing windows and dependencies

None.

## What a test must construct

Each reason constructed separately, with the `publish_budget_expired` and deferred-unit constructions listed in the investigation log: `crates/daemon/src/source_capture_tests.rs:3496` (cited and descriptor not found), `crates/daemon/src/source_capture_tests.rs:3646` (source capture off, hashed id, plain, unborn, and SHA-256 roots, detached HEAD); the hashed fragment id the checkpoint writes passes the shape test (`crates/daemon/src/memory_capture.rs:1587`).

## Investigation log

### Q: Is `publish_budget_expired` constructible?

- Sources examined: the U4 code and tests.
- Findings: yes. `a_publish_past_its_budget_releases_the_submit_at_the_deadline` (`crates/daemon/src/source_capture_tests.rs:4205`) parks a source job past its budget, and `a_queue_full_refusal_settles_at_once_and_hashed_ids_skip_the_wait` (`crates/daemon/src/source_capture_tests.rs:4259`) refuses a second session's handoff; both commit `publish_budget_expired` decisions with no citation event and count `decisions_publish_budget_expired`. `a_completed_publish_with_a_deferred_unit_reports_descriptor_not_found` (`crates/daemon/src/source_capture_tests.rs:4483`) constructs the `Ended { Completed }` with a deferred unit case, and `a_terminal_or_absent_entry_settles_without_waiting` (`crates/daemon/src/source_capture/settlement.rs:312`) maps `Ended { Deadline }` and `Refused` to `BudgetExpired`.
- Conclusion: resolved.
