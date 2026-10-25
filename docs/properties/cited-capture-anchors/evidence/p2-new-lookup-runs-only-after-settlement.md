# p2-new-lookup-runs-only-after-settlement

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), obligation for constraints C7 and C8. Activated by the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).

## Evidence trail

A native capture plan holds the project's pending jobs for the harness (`next_native_plan`), so one submit can carry jobs of several sessions, and each job's lookup reads its own session's lineages (`message_lineage`). The submit handler therefore observes the record of every distinct job session under one lock and settles each observation: `capture_settlements` (`crates/daemon/src/memory_capture/native.rs:124`, awaited at `:394`) builds each key with `settlement_key` (`crates/daemon/src/memory_capture/cite.rs:119`) from the job's project, harness, and session id, before `kernel_routes::blocking`, whose closure reaches `publish_captured_memories` and its lazy `Citation::lookup` (`crates/daemon/src/memory_capture.rs:1278`). Each wait returns only when the observed entry is absent, `Ended`, or `Refused`, when the entry is written again (the job's end, a refusal, or a newer acceptance), or past `accepted_at + CAPTURE_BUDGET` (`settle`, `crates/daemon/src/source_capture/settlement.rs:204`). Each job's outcome travels into `publish` as `SettlementOutcome`, so the lookup's empty result maps to `publish_budget_expired` after `BudgetExpired` and to `descriptor_not_found` after `Settled`. A prepared plan replayed by a later claim reads the record directly (`SourceCapture::settlement_now`, `crates/daemon/src/source_capture.rs:599`), and a `Running` entry then reads as `BudgetExpired`.

The record's writers sit at the handoff exits: `Running` in `into_job` (`crates/daemon/src/source_capture.rs:710`), `Refused` in `refuse_handoff` (`:572`) for the queue-full refusal and for the shutting-down refusal, which `TaskAdmission::try_spawn` (`crates/daemon/src/lib.rs:591`) reports before the unrun job drops, `Ended` in `CaptureJob::end` after the gate is released and in `run` for a failed join, and `ActiveSlot::drop` (`crates/daemon/src/source_capture.rs:470`) ends a still-`Running` entry as `Discarded`, so a free slot leaves no `Running` entry. A job that panics after releasing the gate drops its slot during the unwind, so its entry reads `Ended { Discarded }` briefly before `run` records `Ended { WorkerPanicked }`; both settle as `Settled`. A terminal write names its acceptance; a write from a job whose entry a later acceptance or refusal overwrote is ignored, and a poisoned record restarts empty with its acceptance count kept.

`p2-deferred-source-units-publish-on-a-later-pass` shows a completed publish can leave a unit deferred, so settlement is job end, not descriptor presence.

Recorded imprecision: a submit that arrives before the pass's handoff becomes a job reads `absent` and reports `descriptor_not_found`; when pass N+1 of a session is refused while pass N runs, the key reads `Refused` and a submit quoting pass N reports `publish_budget_expired`. Neither produces a wrong citation. A shutting-down refusal is recorded, but the daemon cancel that closes admission also ends every waiting submit first, so no submit reports it.

## Failure scenario

A `descriptor_not_found` decision whose descriptor committed from a job still running within its deadline.

## Timing windows and dependencies

A slow source job and a fast submit; a submit on another session of the same project whose lease carries the job.

## What a test must construct

- `a_submit_during_the_publish_waits_outside_the_gate_and_then_cites` (`crates/daemon/src/source_capture_tests.rs:4093`): with the source job parked, the submit waits, the test-only `citation_publishes` counter stays 0 and no decision row exists while the entry is `Running`; after release the decision is `cited`.
- `a_submit_on_another_session_waits_on_each_job_session_it_carries` (`crates/daemon/src/source_capture_tests.rs:4158`): a second session's submit whose batch carries the parked session's job waits on that session's entry and cites the job after release.
- `a_completed_publish_with_a_deferred_unit_reports_descriptor_not_found` (`crates/daemon/src/source_capture_tests.rs:4483`): a 33-unit window ends `Ended { Completed }` with one unit deferred; the submit arms no wait, the published unit cites, and the deferred unit reports `descriptor_not_found`.
- `a_terminal_or_absent_entry_settles_without_waiting` (`crates/daemon/src/source_capture/settlement.rs:241`) and `a_later_acceptance_after_the_awaited_end_reads_as_settled` (`crates/daemon/src/source_capture/settlement.rs:393`): absent, terminal, and superseded entries return without advancing the paused clock.

## Investigation log

### Q: Can the lookup run while the entry is Running within its deadline?

- Sources examined: the submit handler, `settle`, the counter assertion in `crates/daemon/src/source_capture_tests.rs:4093`, the cross-session test.
- Findings: no for a submitted job; the blocking hop starts only after every job session's `settle` returns, and `settle` returns for an unchanged `Running` entry only at the deadline. A replayed prepared plan runs its lookup at once and reads a `Running` entry as `BudgetExpired` (`Settlements::now`, `the_current_outcome_reads_a_running_publish_as_budget_expired`), so its empty lookup reports `publish_budget_expired`, which the guarantee states.
- Conclusion: resolved.
