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
  `chrono::Local` and maps `None` to `i64::MAX` (`:735-737`). The conditional-note
  path calls `next_due_at_ms` through
  [`next_conditional_note_check_due_at`][note-due] with a clamp and jitter
  (`:642-671`). This record witnesses the scheduler consumer; the note
  consumer has the fixture-zone golden and is covered by W6's Exercised.
- [`MemoryClassifierScheduler::tick`][sched-tick] asks the host for
  `scheduled_projects()` and hands them to [`due_projects`][sched-due-projects],
  which calls `next_due` for a project on first sight (`:366-371`) and again
  when its schedule string changes (`:372-377`); a project is due when the
  stored instant is at or before `now_ms` (`:378-380`).
- The production host's [`scheduled_projects`][sched-projects] reads
  `memory_classifier_review_user_memories_schedule` from each bound route's frozen
  config (`:15087`), skips roots whose memories authority is not `MODULE`
  ([`:15095-15100`][sched-authority]), and drops a project with no schedule
  through `let schedule = schedule?;` ([`:15119`][sched-filter]).
- The schedule [defaults to `None`][sched-default], so a default campaign
  yields an empty list, `next_due` is never called, and the loop waits
  [`IDLE_POLL`][sched-idle] (60 s) between ticks.
- Config acceptance runs [`is_valid_conditional_note_cron`][valid] on the schedule
  ([`config.rs:1232`][sched-accept]), so only a parseable expression reaches
  the scheduler.
- The scheduler tests build the state directly. The [`project`][sched-fixture]
  helper activates `MODULE` memories authority on a real store
  ([`activate`][sched-activate]) and returns a `ScheduledProject` with the
  given schedule; [`scripted`][sched-scripted] wraps a list of them in a
  `ScriptedHost`; [`ManualClock`][sched-clock] moves time by hand.
  [`a_task_runs_only_once_its_cron_instant_has_passed`][t-sched-cron] uses
  `*/15 * * * *`, advances 14 then 15 minutes, and asserts the run at the
  15-minute instant (`:1027-1051`). Its siblings cover backlog order, schedule
  change, lease failures, and clock steps (`:1073-1369`).

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
a user tier with `memory_classifier_review_user_memories_schedule` set and a bound
route in `MODULE` authority. No existing test records the marker.

## Investigation log

### Q: Is the enabling state constructible from existing fixtures?

- Sources examined: the scheduler test module
  ([`:805-2599`][sched-tests-span] as a whole), the `project`, `scripted`,
  and `activate` helpers, `ManualClock`, and the production
  `scheduled_projects`.
- Findings: Yes. Every scheduler test already constructs a scheduled project
  with a real cron on a real store and drives `tick` past the instant. The
  marker is an added observation at the `next_due` call, not a new fixture.
- Missing evidence: None for constructibility. Whether the campaign contract
  includes the scheduler path is the human decision the evaluation's bias 4
  names; this record supplies the witness rather than the exemption.
- Conclusion: resolved with answer - constructible; the witness is recorded.

### Q: Is the marker reached? (at the field-jumping head)

- Sources examined: [the marker test][marker-head] and
  [`due_projects`][due-projects-head].
- Findings: The test builds a `MODULE`-authority project with a
  `*/15 * * * *` schedule through the `project` helper, ticks a
  `ManualClock` scheduler once, and reads the scheduler's `next_due` entry:
  it holds the schedule and `T0 + 15 min`, an instant other than `i64::MAX`.
  The assertion message is the constant marker.
- Missing evidence: None.
- Conclusion: resolved with answer - the marker fires.

[sched-idle]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L86
[sched-tick]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L327-L347
[sched-due-projects]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L351-L385
[sched-due]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L735-L737
[sched-clock]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L743-L748
[sched-tests-span]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L805-L2599
[sched-activate]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L905-L927
[sched-fixture]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L929-L937
[sched-scripted]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L939-L952
[t-sched-cron]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L1027-L1051
[sched-projects]: ../../../../../crates/daemon/src/lib.rs#L15084-L15153
[sched-authority]: ../../../../../crates/daemon/src/lib.rs#L15108-L15113
[sched-filter]: ../../../../../crates/daemon/src/lib.rs#L15132
[sched-default]: ../../../../../crates/daemon/src/config.rs#L155
[sched-accept]: ../../../../../crates/daemon/src/config.rs#L1255
[valid]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L622-L632
[occurrence]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L630-L632
[note-due]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L642-L671
[cap]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L31-L34
[marker-head]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L1054-L1071
[due-projects-head]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L351-L385
