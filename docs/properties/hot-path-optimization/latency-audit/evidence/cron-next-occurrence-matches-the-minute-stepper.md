# cron-next-occurrence-matches-the-minute-stepper

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The cron search steps one epoch minute at a time up to a four-year cap, on
the scheduler task. A field-jumping replacement is the obvious optimization.
The wildcard pass asked what the stepper's answer depends on and found three
things a jumper can get wrong: DST handling through local civil fields, Vixie
day semantics, and unsatisfiable-but-valid expressions that must return
`None` at the cap rather than an instant or a panic.

## Evidence trail

- [`parse_cron`][parse] accepts five numeric fields with day-of-month `1..31`
  independent of month, and sets `dom_restricted` and `dow_restricted` from a
  leading `*`. [`matches_day`][vixie] implements Vixie OR semantics when both
  are restricted.
- [`next_occurrence`][stepper] starts at the minute boundary after
  `after_ms`, caps at `after_ms + min(MAX_SEARCH_MS, max_search_ms)`,
  evaluates `tz.timestamp_millis_opt(cursor_ms).single()?` per minute so an
  unrepresentable instant returns `None`, and matches minute, hour, month, and
  day on the local civil fields. Its doc says evaluating each epoch minute by
  civil fields handles DST transitions.
- [`MAX_SEARCH_MS`][cap] is `4 * 366 * 24 * 60 * MINUTE_MS`, so an
  unsatisfiable expression pays 2_108_160 iterations per call.
- [`next_cron_occurrence`][occurrence] applies the parser's own cap and
  filters `ms != 0`; [`is_valid_smart_note_cron`][valid] is
  `parse_cron(...).is_some()`. Smart notes use a smaller ceiling through
  `next_due_at_ms` at [`:236-239`][note-cap] with the same zero filter.
- The dreamer scheduler's [`next_due`][sched-due] calls
  `next_cron_occurrence(schedule, now_ms, &chrono::Local)` and treats `None`
  as `i64::MAX`. The schedule [defaults to `None`][sched-default]; the config
  key accepts any string [`is_valid_smart_note_cron`][sched-accept] accepts
  and warns otherwise.
- Existing checks: [Vixie `*`-prefix semantics][t-vixie], [`None` at `i64`
  extremes][t-extreme], and the [fixture-timezone golden][t-golden-cron].

## Failure scenario

A jumper computes the next civil match and converts once to an instant. On a
spring-forward day `30 2 * * *` has no civil 02:30, which the stepper skips
by construction; a jumper that builds the instant from fields may fire at an
adjacent hour or a different day. On fall-back the stepper returns the earlier
of the two instants with the same civil time; a jumper may return the later.
For `0 0 30 2 *` the stepper returns `None` after the cap; a jumper that
searches by field may loop or return a February 30 that normalizes into
March. The scheduler consumes the value directly as its due time.

## Timing windows and dependencies

None in time inside the function. The stepper runs synchronously on the
scheduler task, so an unsatisfiable schedule costs the full cap on every
`next_due` call; the property forbids running past that cap.

## What a test must construct

A differential against [`next_occurrence`][stepper] as frozen at HEAD over
`Local` with a DST-bearing zone and over `Utc`, for `30 2 * * *` on
spring-forward, `0 0 30 2 *`, `0 0 31 4,6,9,11 *`, `0 0 29 2 *`, and
`after_ms` at the `i64` extremes, comparing `Option<i64>`. The
[wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) list the
Vixie test, the extreme-instant test, and the golden; none covers DST or an
unsatisfiable expression. Reachability is explicit-config-only: the dreamer
schedule defaults to `None` and smart notes need a cron on the note.

The frozen copy is a test-only function kept because the change replaces
`next_occurrence` itself; the live function cannot remain the oracle.

## Investigation log

### Q: Should validation reject calendar-impossible dates instead?

- Sources examined: [`parse_cron`][parse], [`sched-accept`][sched-accept],
  [`is_valid_smart_note_cron`][valid].
- Findings: Validation is a grammar check; `0 0 30 2 *` is accepted and never
  fires. Rejecting it changes config acceptance and the warning surface,
  which is outside a latency change.
- Missing evidence: A product decision on config strictness.
- Conclusion: needs human input.

[cap]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L31-L34
[parse]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L125-L145
[vixie]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L150-L160
[stepper]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L163-L192
[valid]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L205-L210
[occurrence]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L212-L218
[note-cap]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L236-L239
[t-golden-cron]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L1126
[t-extreme]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L1580
[t-vixie]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L1594
[sched-due]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L412-L416
[sched-default]: ../../../../../crates/daemon/src/config.rs#L127
[sched-accept]: ../../../../../crates/daemon/src/config.rs#L881-L895
