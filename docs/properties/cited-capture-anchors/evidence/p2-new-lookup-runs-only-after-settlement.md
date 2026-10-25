# p2-new-lookup-runs-only-after-settlement

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), obligation for constraints C7 and C8. Activated by the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).

## Evidence trail

The submit handler awaits `capture_settlement` (`crates/daemon/src/memory_capture/native.rs:122`, awaited at `:394`) before `kernel_routes::blocking`, whose closure reaches `publish_captured_memories` and its lazy `Citation::lookup` (`crates/daemon/src/memory_capture.rs:1257`). The wait returns only when the session's entry is absent, `Ended`, `Refused`, or past `accepted_at + CAPTURE_BUDGET` (`settle`, `crates/daemon/src/source_capture/settlement.rs:151`). The outcome travels into `publish` as `SettlementOutcome`, so the lookup's empty result maps to `publish_budget_expired` after `BudgetExpired` and to `descriptor_not_found` after `Settled`.

The record's writers sit at the handoff exits: `Running` in `into_job` (`crates/daemon/src/source_capture.rs:697`), `Refused` in `refuse_handoff` (`:572`) for the queue-full refusal and the shutting-down refusal (`crates/daemon/src/lib.rs:9171`), `Ended` in `CaptureJob::end` and in `run` for a failed join, and `ActiveSlot::drop` (`crates/daemon/src/source_capture.rs:470`) ends a still-`Running` entry as `Discarded`, so a free slot leaves no `Running` entry. A terminal write names its acceptance; a write from a job whose entry a later acceptance or refusal overwrote is ignored.

`p2-deferred-source-units-publish-on-a-later-pass` shows a completed publish can leave a unit deferred, so settlement is job end, not descriptor presence.

Recorded imprecision: a submit that arrives before the pass's handoff becomes a job reads `absent` and reports `descriptor_not_found`; when pass N+1 of a session is refused while pass N runs, the key reads `Refused` and a submit quoting pass N reports `publish_budget_expired`. Neither produces a wrong citation.

## Failure scenario

A `descriptor_not_found` decision whose descriptor committed from a job still running within its deadline.

## Timing windows and dependencies

A slow source job and a fast submit.

## What a test must construct

- `a_submit_during_the_publish_waits_outside_the_gate_and_then_cites` (`crates/daemon/src/source_capture_tests.rs:3961`): with the source job parked, the submit waits, the test-only `citation_publishes` counter stays 0 and no decision row exists while the entry is `Running`; after release the decision is `cited`.
- `a_completed_publish_with_a_deferred_unit_reports_descriptor_not_found` (`crates/daemon/src/source_capture_tests.rs:4287`): a 33-unit window ends `Ended { Completed }` with one unit deferred; the submit arms no wait, the published unit cites, and the deferred unit reports `descriptor_not_found`.
- `a_terminal_or_absent_entry_settles_without_waiting` (`crates/daemon/src/source_capture/settlement.rs:200`): absent and terminal entries return without advancing the paused clock.

## Investigation log

### Q: Can the lookup run while the entry is Running within its deadline?

- Sources examined: the submit handler, `settle`, the counter assertion in `crates/daemon/src/source_capture_tests.rs:3961`.
- Findings: no; the blocking hop starts only after `settle` returns, and `settle` returns for `Running` only at the deadline.
- Conclusion: resolved.
