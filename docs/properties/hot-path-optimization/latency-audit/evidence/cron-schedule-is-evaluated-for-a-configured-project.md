# cron-schedule-is-evaluated-for-a-configured-project

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The area's [portfolio evaluation](../portfolio-evaluation.md#gaps-queued)
queued gap 2: W6 is an explicit-config-only `always` record and, on a
default campaign, the scheduler that consumes `next_occurrence` never sees a
schedule, so W6's differential runs on no instant the daemon uses. The
disposition step found the enabling state constructible from the scheduler's
own test fixtures, so a `sometimes` witness is recorded for the scheduler
consumer.

## Evidence trail

- `next_occurrence` has two production consumers. The scheduler's
  [`next_due`][sched-due] calls [`next_cron_occurrence`][occurrence] with
  `chrono::Local` and maps `None` to `i64::MAX` (`:414-416`). The smart-note
  path calls `next_due_at_ms` through
  [`next_smart_note_check_due_at`][note-due] with a clamp and jitter
  (`:228-239`). This record witnesses the scheduler consumer; the note
  consumer has the fixture-zone golden and is covered by W6's Exercised.
- [`DreamerScheduler::tick`][sched-tick] asks the host for
  `scheduled_projects()` and hands them to [`due_projects`][sched-due-projects],
  which calls `next_due` for a project on first sight (`:277-282`) and again
  when its schedule string changes (`:283-288`); a project is due when the
  stored instant is at or before `now_ms` (`:289-291`).
- The production host's [`scheduled_projects`][sched-projects] reads
  `dreamer_review_user_memories_schedule` from each bound route's frozen
  config (`:13991`), skips roots whose memories authority is not `MODULE`
  ([`:13998-14003`][sched-authority]), and drops a project with no schedule
  through `schedule: schedule?` ([`:14023`][sched-filter]).
- The schedule [defaults to `None`][sched-default], so a default campaign
  yields an empty list, `next_due` is never called, and the loop waits
  [`IDLE_POLL`][sched-idle] (60 s) between ticks.
- Config acceptance runs [`is_valid_smart_note_cron`][valid] on the schedule
  ([`config.rs:887`][sched-accept]), so only a parseable expression reaches
  the scheduler.
- The scheduler tests build the state directly. The [`project`][sched-fixture]
  helper activates `MODULE` memories authority on a real store
  ([`activate`][sched-activate]) and returns a `ScheduledProject` with the
  given schedule; [`scripted`][sched-scripted] wraps a list of them in a
  `ScriptedHost`; [`ManualClock`][sched-clock] moves time by hand.
  [`a_task_runs_only_once_its_cron_instant_has_passed`][t-sched-cron] uses
  `*/15 * * * *`, advances 14 then 15 minutes, and asserts the run at the
  15-minute instant (`:680-704`). Its siblings cover backlog order, schedule
  change, lease failures, and clock steps (`:707-1257`).

## Failure scenario

Not a violation; a coverage gap. A default campaign ticks the scheduler on
an empty list forever. W6's differential, run as a pure-function test over
expressions, still holds, but nothing shows the scheduler consumed an
instant the differential covers; a replacement wired into `next_due` with a
different `Local` handling or a dropped `ms != 0` filter passes the campaign.

## Timing windows and dependencies

None in time beyond the tick reaching a `now_ms` at which the project has an
occurrence inside [`MAX_SEARCH_MS`][cap]. An unsatisfiable expression makes
`next_due` return `i64::MAX`, which the marker treats as no evaluation
result; the marker fires on a finite instant only.

## What a test must construct

A `ScriptedHost` with one project built by the `project` helper and a valid
schedule, a `ManualClock`, and one `tick`. At the `next_due` call inside
`due_projects`, record under the constant marker the project, the schedule
string, `now_ms`, and the returned instant, and assert the instant is not
`i64::MAX`. The marker asserts the input and that the evaluation happened; it
does not assert the slot ran (the scheduler tests do that) or the instant's
correctness (W6 does that). In a daemon-level campaign the same marker needs
a user tier with `dreamer_review_user_memories_schedule` set and a bound
route in `MODULE` authority. No existing test records the marker.

## Investigation log

### Q: Is the enabling state constructible from existing fixtures?

- Sources examined: the scheduler test module
  ([`:458-1292`][sched-tests-span] as a whole), the `project`, `scripted`,
  and `activate` helpers, `ManualClock`, and the production
  `scheduled_projects`.
- Findings: Yes. Every scheduler test already constructs a scheduled project
  with a real cron on a real store and drives `tick` past the instant. The
  marker is an added observation at the `next_due` call, not a new fixture.
- Missing evidence: None for constructibility. Whether the campaign contract
  includes the scheduler path is the human decision the evaluation's bias 4
  names; this record supplies the witness rather than the exemption.
- Conclusion: resolved with answer - constructible; the witness is recorded.

[sched-idle]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L36
[sched-tick]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L244-L261
[sched-due-projects]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L265-L296
[sched-due]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L412-L416
[sched-clock]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L418-L427
[sched-tests-span]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L458-L1292
[sched-activate]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L562-L584
[sched-fixture]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L586-L593
[sched-scripted]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L595-L600
[t-sched-cron]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L680-L704
[sched-projects]: ../../../../../crates/daemon/src/lib.rs#L14032-L14081
[sched-authority]: ../../../../../crates/daemon/src/lib.rs#L14051-L14056
[sched-filter]: ../../../../../crates/daemon/src/lib.rs#L14076
[sched-default]: ../../../../../crates/daemon/src/config.rs#L127
[sched-accept]: ../../../../../crates/daemon/src/config.rs#L887
[valid]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L205-L210
[occurrence]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L212-L218
[note-due]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L228-L239
[cap]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L31-L34
